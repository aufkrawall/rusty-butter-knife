//! Deletion layer: ownership helpers, extended-length paths, attribute
//! clearing, reboot-time scheduling, and candidate deletion.
//! Ports of `takeOwnershipIfRequested`, `extendedLengthPath`,
//! `clearBlockingAttributes`, `scheduleDeleteOne/Tree`, `deleteCandidate`.

use std::os::windows::fs::MetadataExt;
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
    let takeown_exe = match crate::sysinfo::system_dir_file("takeown.exe") {
        Ok(p) => p,
        Err(e) => {
            log_line("ERROR", &format!("Ownership step skipped: {e}"));
            return;
        }
    };
    let icacls_exe = match crate::sysinfo::system_dir_file("icacls.exe") {
        Ok(p) => p,
        Err(e) => {
            log_line("ERROR", &format!("ACL grant step skipped: {e}"));
            return;
        }
    };
    // '/D' expects a locale-specific letter for "Yes" (German wants J,
    // French O, ...). Try variants until accepted. To avoid N full 120 s
    // round-trips per candidate (audit finding: ownership retries), the
    // winning letter index is memoized process-wide so later candidates go
    // straight there. Native SID/security APIs remain deferred debt.
    const YES_LETTERS: &[&str] = &["Y", "J", "O", "S"];
    static WINNING_LETTER: std::sync::OnceLock<std::sync::atomic::AtomicUsize> =
        std::sync::OnceLock::new();
    let winning = WINNING_LETTER.get_or_init(std::sync::atomic::AtomicUsize::default);
    let mut order: Vec<usize> = (0..YES_LETTERS.len()).collect();
    let start_idx = winning.load(std::sync::atomic::Ordering::Relaxed);
    let rotate = start_idx.min(order.len() - 1);
    order.rotate_left(rotate);

    let mut takeown_ok = false;
    for idx in order {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if crate::procs::run_shell_command(
            &crate::util::join_command(&[
                takeown_exe.clone(),
                "/F".into(),
                target.into(),
                "/A".into(),
                "/R".into(),
                "/D".into(),
                YES_LETTERS[idx].into(),
            ]),
            "takeown",
            true,
        ) {
            takeown_ok = true;
            winning.store(idx, std::sync::atomic::Ordering::Relaxed);
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
    if execute && !takeown_ok && !app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
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
    let ext = extended_length_path(path);
    crate::ffi::set_file_attributes_normal(&ext.to_string_lossy())
}

/// Read-only/system attributes make DeleteFileW and removals fail; clear them
/// recursively without following symlinks or crossing reparse points
/// (FS-01). Port of `clearBlockingAttributes`.
fn clear_blocking_attributes(root: &Path) {
    let ext_root = extended_length_path(root);
    set_attrs_normal(&ext_root);
    fn walk_clear(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            set_attrs_normal(&path);
            // Only REAL directories are descended into: reparse points get
            // surface attribute clearing only.
            let attrs = entry.metadata().map(|m| m.file_attributes()).unwrap_or(0);
            let is_real_dir = !crate::fsutil::attrs_are_reparse(attrs)
                && entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_real_dir {
                walk_clear(&path);
            }
        }
    }
    if matches!(
        crate::fsutil::path_kind_no_follow(&ext_root),
        Ok(crate::fsutil::PathKind::Directory)
    ) {
        walk_clear(&ext_root);
    }
}

/// Port of `scheduleDeleteOne`. Successful scheduling is recorded in
/// `RunState.reboot_scheduled_paths` so the post-run check reports PENDING
/// REBOOT only for verifiably successful MoveFileExW calls (REPORT-01).
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
    let key = crate::util::to_lower(&ps);
    app::run_mut(|s| s.reboot_scheduled_paths.insert(key));
    true
}

/// Port of `scheduleDeleteTree`: children first via postorder traversal,
/// abort-aware, never descending through reparse points and never
/// materializing/sorting the whole tree up front (audit finding:
/// traversal cost + FS-01 boundary).
fn schedule_delete_tree(p: &Path) -> bool {
    crate::fsutil::for_each_postorder(p, &mut |path, _kind| {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            app::run_mut(|s| s.aborted = true);
            return false; // stop traversal; caller records partial results
        }
        schedule_delete_one(path); // failures are logged by schedule_delete_one
        true
    })
}

/// Counted recursive removal (std's remove_dir_all does not return a count;
/// the legacy build logs `Removed entries=N`). Reparse entries are removed
/// AS ENTRIES (the link itself), their targets untouched (FS-01).
fn remove_tree_counted(root: &Path) -> Result<u64, std::io::Error> {
    let meta = std::fs::symlink_metadata(root)?;
    if meta.file_attributes() & crate::fsutil::FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_dir() {
        // A junction/dir-symlink needs remove_dir semantics; file symlinks
        // need remove_file.
        const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
        let is_dir_entry =
            (meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0) || meta.is_dir();
        if is_dir_entry {
            std::fs::remove_dir(root)?;
        } else {
            std::fs::remove_file(root)?;
        }
        return Ok(1);
    }
    let mut count: u64 = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let child = entry.path();
        let attrs = entry.metadata().map(|m| m.file_attributes()).unwrap_or(0);
        let is_reparse = crate::fsutil::attrs_are_reparse(attrs);
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_reparse {
            // Remove the reparse ENTRY itself; never its target.
            count += remove_tree_counted(&child)?;
        } else if is_dir {
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

/// Pure decision core of `unsafe_recursive_directory_target` (unit-testable):
/// whole DriverStore package roots and non-allow-listed driver payload folders
/// must never be removed recursively. Fail-closed: when in doubt, return true.
fn unsafe_recursive_directory_decision(
    is_dir: bool,
    full_lower: &str,
    parent_lower: &str,
    leaf_lower: &str,
) -> bool {
    if !is_dir {
        return false;
    }
    let package_root = parent_lower.contains("\\windows\\system32\\driverstore\\filerepository")
        && crate::matching::leaf_has_inf_arch_marker(leaf_lower);
    if package_root {
        return true;
    }
    // Do not recursively delete whole driver payload folders except explicit known bloat folders.
    if full_lower.contains("\\windows\\system32\\driverstore\\filerepository\\") {
        const ALLOWED_DRIVER_STORE_DIRS: &[&str] = &[
            "nvcamera",
            "nvwmi",
            "ansel",
            "display.update",
            "update.core",
        ];
        if !ALLOWED_DRIVER_STORE_DIRS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(leaf_lower))
        {
            return true;
        }
    }
    false
}

/// Port of `unsafeRecursiveDirectoryTarget`: whole DriverStore package roots
/// and non-allow-listed driver payload folders must never be removed
/// recursively. Kind probing is no-follow and single-shot; an UNREADABLE stat
/// or a reparse point fails closed (treated as an unsafe recursive target).
fn unsafe_recursive_directory_target(p: &Path) -> bool {
    let treat_as_directory = !matches!(
        crate::fsutil::path_kind_no_follow(p),
        Ok(crate::fsutil::PathKind::Missing | crate::fsutil::PathKind::File)
    );
    unsafe_recursive_directory_decision(
        treat_as_directory,
        &crate::matching::path_wide_lower(p),
        &crate::util::to_lower(
            &p.parent()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        &crate::util::to_lower(
            &p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    )
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

    // No-follow tri-state existence probe (REPORT-02): genuine absence is
    // SKIP, an inaccessible stat must NOT masquerade as "gone".
    match crate::fsutil::path_kind_no_follow(&c.path) {
        Ok(crate::fsutil::PathKind::Missing) => {
            log_action(
                "DeletePath",
                "SKIP",
                &c.component_key,
                &path_str,
                "Path no longer exists",
            );
            return;
        }
        Err(e) => log_action(
            "DeletePath",
            "WARN",
            &c.component_key,
            &path_str,
            &format!("Existence unverifiable ({e}); attempting deletion anyway"),
        ),
        _ => {}
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

    take_ownership_if_requested(&path_str);

    // Dispatch by NO-FOLLOW kind: reparse entries are removed as entries
    // (link itself), not descended into (FS-01).
    let kind = crate::fsutil::path_kind_no_follow(&c.path).ok();
    let treat_as_directory = match kind {
        Some(crate::fsutil::PathKind::Directory) => true,
        // Reparse entries are removed AS ENTRIES (the link itself), never
        // descended into (FS-01). remove_tree_counted handles junction and
        // file-symlink roots entry-wise; plain DeleteFileW cannot remove a
        // directory junction, so routing reparse candidates here would
        // guarantee a failed delete and push the work into the reboot-
        // scheduling fallback.
        Some(crate::fsutil::PathKind::Reparse) => true,
        Some(crate::fsutil::PathKind::Missing | crate::fsutil::PathKind::File) => false,
        _ => c.is_directory, // Unverifiable stat: legacy hint as fallback
    };

    let result: Result<(), String> = if treat_as_directory {
        let first = remove_tree_counted(&c.path);
        let final_res = match first {
            Ok(removed) => Ok(removed),
            Err(_) => {
                // Retry once: clear read-only attributes and try the extended-length form.
                let ext = extended_length_path(&c.path);
                clear_blocking_attributes(&ext);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(is_dir: bool, full: &str, parent: &str, leaf: &str) -> bool {
        unsafe_recursive_directory_decision(is_dir, full, parent, leaf)
    }

    #[test]
    fn files_are_never_unsafe_recursive_targets() {
        assert!(!decision(
            false,
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373",
            "c:\\windows\\system32\\driverstore\\filerepository",
            "nv_dispi.inf_amd64_0373"
        ));
    }

    #[test]
    fn package_roots_always_blocked() {
        assert!(decision(
            true,
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373",
            "c:\\windows\\system32\\driverstore\\filerepository",
            "nv_dispi.inf_amd64_0373"
        ));
        assert!(decision(
            true,
            "c:\\windows\\system32\\driverstore\\filerepository\\nvhda.inf_arm64_9074",
            "c:\\windows\\system32\\driverstore\\filerepository",
            "nvhda.inf_arm64_9074"
        ));
    }

    #[test]
    fn non_allowlisted_driver_store_dirs_blocked() {
        assert!(decision(
            true,
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\DisplayDriver",
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373",
            "DisplayDriver"
        ));
        assert!(decision(
            true,
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\system32",
            "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373",
            "system32"
        ));
    }

    #[test]
    fn allowlisted_driver_store_dirs_permitted() {
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\nvwmi", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "nvwmi"), "nvwmi");
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\display.update", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "display.update"), "display.update");
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\update.core", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "update.core"), "update.core");
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\ansel", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "ansel"), "ansel");
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\nvcamera", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "nvcamera"), "nvcamera");
        // Verify case-insensitivity on allowlisted directories
        assert!(!decision(true, "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373\\NvCamera", "c:\\windows\\system32\\driverstore\\filerepository\\nv_dispi.inf_amd64_0373", "NvCamera"), "NvCamera mixed case");
    }

    #[test]
    fn ordinary_dirs_outside_driver_store_permitted() {
        assert!(!decision(
            true,
            "C:\\Program Files\\NVIDIA Corporation\\Installer2",
            "C:\\Program Files\\NVIDIA Corporation",
            "Installer2"
        ));
    }
}
