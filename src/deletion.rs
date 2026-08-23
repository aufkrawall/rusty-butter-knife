//! Deletion layer: ownership helpers, extended-length paths, attribute
//! clearing, reboot-time scheduling, and candidate deletion.
//! Ports of `takeOwnershipIfRequested`, `extendedLengthPath`,
//! `clearBlockingAttributes`, `scheduleDeleteOne/Tree`, `deleteCandidate`.

use std::path::{Path, PathBuf};

use crate::app;
use crate::logging::{log_action, log_line};
use crate::winfmt::format_win_error;

/// Port of `takeOwnershipIfRequested` (takeown/icacls via absolute System32
/// paths; grants via well-known SID so localized group names cannot fail).
fn take_ownership_if_requested(target: &str) {
    let (take_ownership, execute) = app::opts(|o| (o.take_ownership, o.execute));
    if !take_ownership {
        return;
    }
    let takeown_exe = crate::sysinfo::system_dir_file("takeown.exe");
    let icacls_exe = crate::sysinfo::system_dir_file("icacls.exe");
    // '/D' expects a locale-specific letter for "Yes" (German wants J,
    // French O, ...). Try the common variants until one is accepted.
    const YES_LETTERS: &[&str] = &["Y", "J", "O", "S"];
    let mut takeown_ok = false;
    for letter in YES_LETTERS {
        if crate::procs::run_shell_command(
            &crate::util::join_command(&[
                takeown_exe.clone(),
                "/F".into(),
                target.into(),
                "/A".into(),
                "/R".into(),
                "/D".into(),
                (*letter).into(),
            ]),
            "takeown",
            true,
        ) {
            takeown_ok = true;
            break;
        }
        if !execute {
            break; // dry-run: one representative command suffices
        }
    }
    // Grant via well-known SID; localized group names ("Administratoren", ...)
    // would fail on non-English systems.
    crate::procs::run_shell_command(
        &crate::util::join_command(&[
            icacls_exe,
            target.into(),
            "/grant".into(),
            "*S-1-5-32-544:F".into(),
            "/T".into(),
            "/C".into(),
        ]),
        "icacls",
        true,
    );
    if execute && !takeown_ok {
        log_line(
            "WARN",
            "takeown did not report success; the '/D' yes-letter may differ on this locale.",
        );
    }
}

/// `\\?\`-prefixed variant to sidestep MAX_PATH limits on deep DriverStore
/// trees. Port of `extendedLengthPath`. UNC or relative paths are untouched.
fn extended_length_path(p: &Path) -> PathBuf {
    let w = p.to_string_lossy();
    if w.starts_with("\\\\?\\") {
        return p.to_path_buf();
    }
    let bytes = w.as_bytes();
    if w.len() >= 2 && bytes[1] == b':' {
        return PathBuf::from(format!("\\\\?\\{w}"));
    }
    p.to_path_buf()
}

fn set_attrs_normal(path: &Path) -> bool {
    crate::ffi::set_file_attributes_normal(&path.to_string_lossy())
}

/// Read-only/system attributes make DeleteFileW and removals fail; clear them
/// recursively without following symlinks. Port of `clearBlockingAttributes`.
fn clear_blocking_attributes(root: &Path) {
    set_attrs_normal(root);
    fn walk_clear(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            set_attrs_normal(&path);
            let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_real_dir {
                walk_clear(&path);
            }
        }
    }
    if root.is_dir() {
        walk_clear(root);
    }
}

/// Port of `scheduleDeleteOne`.
fn schedule_delete_one(p: &Path) -> bool {
    let ps = p.to_string_lossy().into_owned();
    let ext = extended_length_path(p);
    let mut ok = crate::ffi::move_file_ex_delay_until_reboot(&ps);
    if !ok {
        ok = crate::ffi::move_file_ex_delay_until_reboot(&ext.to_string_lossy());
    }
    if !ok {
        log_action(
            "ScheduleDelete",
            "WARN",
            "RebootDelete",
            &ps,
            &format_win_error(crate::ffi::current_last_error()),
        );
        return false;
    }
    log_action(
        "ScheduleDelete",
        "INFO",
        "RebootDelete",
        &ps,
        "Pending delete at reboot",
    );
    true
}

fn collect_paths_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_real_dir {
            collect_paths_recursive(&path, out);
        }
        out.push(path);
    }
}

/// Port of `scheduleDeleteTree`: children first, deepest paths first.
fn schedule_delete_tree(p: &Path) -> bool {
    let mut all_ok = true;
    if p.is_dir() {
        let mut paths: Vec<PathBuf> = Vec::new();
        collect_paths_recursive(p, &mut paths);
        paths.sort_by_key(|b| std::cmp::Reverse(b.to_string_lossy().len()));
        for child in &paths {
            if !child.exists() {
                continue; // vanished already
            }
            all_ok = schedule_delete_one(child) && all_ok;
        }
    }
    if p.exists() {
        all_ok = schedule_delete_one(p) && all_ok;
    }
    all_ok
}

/// Counted recursive removal (std's remove_dir_all does not return a count;
/// the legacy build logs `Removed entries=N`). Symlinks removed as files.
fn remove_tree_counted(root: &Path) -> Result<u64, std::io::Error> {
    let meta = std::fs::symlink_metadata(root)?;
    if meta.is_symlink() || !meta.is_dir() {
        std::fs::remove_file(root)?;
        return Ok(1);
    }
    let mut count: u64 = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let child = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            count += remove_tree_counted(&child)?;
        } else {
            std::fs::remove_file(&child)?;
            count += 1;
        }
    }
    std::fs::remove_dir(root)?;
    count += 1; // the directory itself, matching fs::remove_all semantics
    Ok(count)
}

/// Port of `unsafeRecursiveDirectoryTarget`: whole DriverStore package roots
/// and non-allow-listed driver payload folders must never be removed recursively.
fn unsafe_recursive_directory_target(p: &Path) -> bool {
    if !p.is_dir() {
        return false;
    }
    if crate::matching::is_driver_store_package_root_name(p) {
        return true;
    }
    let full = crate::matching::path_wide_lower(p);
    // Do not recursively delete whole driver payload folders except explicit known bloat folders.
    if full.contains("\\windows\\system32\\driverstore\\filerepository\\") {
        const ALLOWED_DRIVER_STORE_DIRS: &[&str] = &[
            "nvcamera",
            "nvwmi",
            "ansel",
            "display.update",
            "update.core",
        ];
        let leaf = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let leaf_lower = crate::util::to_lower(&leaf);
        if !ALLOWED_DRIVER_STORE_DIRS.contains(&leaf_lower.as_str()) {
            return true;
        }
    }
    false
}

/// Port of `deleteCandidate`. Safety predicates run first — always.
pub fn delete_candidate(index: usize) {
    let Some(c) = app::run(|s| s.candidates.get(index).cloned()) else {
        return;
    };
    let path_str = c.path.to_string_lossy().into_owned();

    if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
        app::run_mut(|s| s.aborted = true);
        return;
    }

    if unsafe_recursive_directory_target(&c.path) {
        log_action(
            "DeletePath",
            "SKIP",
            &c.component_key,
            &path_str,
            "Unsafe recursive directory target",
        );
        return;
    }

    let execute = app::opts(|o| o.execute);
    if !execute {
        log_action(
            "DeletePath",
            "DRYRUN",
            &c.component_key,
            &path_str,
            if c.is_directory { "directory" } else { "file" },
        );
        return;
    }

    if !c.path.exists() {
        log_action(
            "DeletePath",
            "SKIP",
            &c.component_key,
            &path_str,
            "Path no longer exists",
        );
        return;
    }

    take_ownership_if_requested(&path_str);

    let result: Result<(), String> = if c.is_directory {
        let first = remove_tree_counted(&c.path);
        let final_res = match first {
            Ok(removed) => Ok(removed),
            Err(_) => {
                // Retry once: clear read-only attributes and try the extended-length form.
                clear_blocking_attributes(&c.path);
                let ext = extended_length_path(&c.path);
                remove_tree_counted(&ext)
            }
        };
        match final_res {
            Ok(removed) => {
                log_action(
                    "DeletePath",
                    "INFO",
                    &c.component_key,
                    &path_str,
                    &format!("Removed entries={removed}"),
                );
                Ok(())
            }
            Err(e) => Err(format!("Delete failed: {e}")),
        }
    } else {
        match crate::ffi::delete_file_raw(&path_str) {
            Ok(()) => {
                log_action(
                    "DeletePath",
                    "INFO",
                    &c.component_key,
                    &path_str,
                    "Deleted file",
                );
                Ok(())
            }
            Err(del_err) => {
                const ERROR_ACCESS_DENIED: u32 = 5;
                const ERROR_FILE_READ_ONLY: u32 = 6009;
                if del_err == ERROR_ACCESS_DENIED || del_err == ERROR_FILE_READ_ONLY {
                    // Likely read-only attribute or a >MAX_PATH path; clear + retry.
                    set_attrs_normal(&c.path);
                    let ext = extended_length_path(&c.path);
                    match crate::ffi::delete_file_raw(&ext.to_string_lossy()) {
                        Ok(()) => {
                            log_action(
                                "DeletePath",
                                "INFO",
                                &c.component_key,
                                &path_str,
                                "Deleted file",
                            );
                            Ok(())
                        }
                        Err(e2) => Err(format!("Delete failed: {}", format_win_error(e2))),
                    }
                } else {
                    Err(format!("Delete failed: {}", format_win_error(del_err)))
                }
            }
        }
    };

    if result.is_ok() {
        return;
    }

    let detail = result.err().unwrap_or_else(|| "Delete failed".to_string());
    log_action("DeletePath", "WARN", &c.component_key, &path_str, &detail);

    if app::opts(|o| o.schedule_locked_for_reboot) {
        schedule_delete_tree(&c.path);
    }
}
