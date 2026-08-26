//! Candidate discovery: search roots, pruned traversal, nested-candidate
//! collapsing, previous-log tallying, and the candidates section of the log.
//! Ports of `getExistingRoots`, `discoverCandidates`, `collapseNestedCandidates`,
//! `tallyPreviousLogs`, `writeCandidatesCsv`.

use std::collections::HashSet;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::app;
use crate::logging::{append_utf8_file, log_line};
use crate::matching::match_component_for_path;
use crate::matching::should_prune_traversal;
use crate::types::Candidate;
use crate::util::{ends_with_no_case, to_lower, trim};

/// Lowercased wide-string view of a path (port of `.wstring()` + toLower).
pub fn path_lower(p: &Path) -> String {
    to_lower(&p.to_string_lossy())
}

fn add_env_root(roots: &mut Vec<PathBuf>, env: &str, suffix: &str) {
    if let Ok(base) = std::env::var(env) {
        if base.is_empty() {
            return;
        }
        let p = PathBuf::from(&base).join(suffix);
        // No-follow kind check: a junctioned "root" must not redirect the
        // scan into some unrelated target tree (FS-01).
        if matches!(
            crate::fsutil::path_kind_no_follow(&p),
            Ok(crate::fsutil::PathKind::Directory)
        ) {
            roots.push(p);
        }
    }
}

/// Port of `getExistingRoots`.
pub fn get_existing_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    add_env_root(&mut roots, "ProgramFiles", "NVIDIA Corporation");
    add_env_root(&mut roots, "ProgramFiles(x86)", "NVIDIA Corporation");
    // CUDA Toolkit is developer tooling, not NVIDIA driver bloat. Do not traverse it by default.
    add_env_root(&mut roots, "ProgramData", "NVIDIA");
    add_env_root(&mut roots, "ProgramData", "NVIDIA Corporation");

    if let Ok(system_root) = std::env::var("SystemRoot") {
        let repo = PathBuf::from(&system_root)
            .join("System32")
            .join("DriverStore")
            .join("FileRepository");
        if let Ok(entries) = std::fs::read_dir(&repo) {
            for entry in entries.flatten() {
                let ok_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !ok_dir {
                    continue; // skip sidecar .ini files etc.
                }
                let leaf = to_lower(&entry.file_name().to_string_lossy());
                if leaf.starts_with("nv") && crate::matching::leaf_has_inf_arch_marker(&leaf) {
                    roots.push(entry.path());
                }
            }
        }
    }

    // Use %SystemDrive% instead of hardcoding C:.
    let sys_drive = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_string())
        .chars()
        .take(2)
        .collect::<String>();
    let users = PathBuf::from(if sys_drive.len() == 2 {
        sys_drive
    } else {
        "C:".to_string()
    })
    .join("Users");
    if let Ok(entries) = std::fs::read_dir(&users) {
        for entry in entries.flatten() {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let name = to_lower(&entry.file_name().to_string_lossy());
            if name == "default"
                || name == "default user"
                || name == "public"
                || name == "all users"
            {
                continue;
            }
            let user_roots = [
                entry.path().join("AppData").join("Local").join("NVIDIA"),
                entry
                    .path()
                    .join("AppData")
                    .join("Local")
                    .join("NVIDIA Corporation"),
                entry.path().join("AppData").join("Roaming").join("NVIDIA"),
                entry
                    .path()
                    .join("AppData")
                    .join("Roaming")
                    .join("NVIDIA Corporation"),
            ];
            for p in user_roots {
                if matches!(
                    crate::fsutil::path_kind_no_follow(&p),
                    Ok(crate::fsutil::PathKind::Directory)
                ) {
                    roots.push(p);
                }
            }
        }
    }

    // Deterministic order identical in spirit to the C++ sort+unique on paths.
    let mut keys: Vec<String> = roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().map(PathBuf::from).collect()
}

/// Port of `isParentOfOrEqual`. Canonicalize-with-fallback mirrors
/// weakly_canonical: existing prefixes resolve to their real casing/path.
fn is_parent_of_or_equal(parent: &Path, child: &Path) -> bool {
    let canon = |p: &Path| -> String {
        match p.canonicalize() {
            Ok(c) => c.to_string_lossy().into_owned(),
            Err(_) => p.to_string_lossy().into_owned(),
        }
    };
    let mut p = to_lower(&canon(parent));
    let c = to_lower(&canon(child));
    if p == c {
        return true;
    }
    if !p.is_empty() && !p.ends_with('\\') {
        p.push('\\');
    }
    c.starts_with(&p)
}

/// Port of `collapseNestedCandidates`.
fn collapse_nested_candidates(candidates: &mut Vec<Candidate>) {
    candidates.sort_by(|a, b| {
        let as_ = a.path.to_string_lossy();
        let bs = b.path.to_string_lossy();
        as_.len().cmp(&bs.len()).then_with(|| as_.cmp(&bs))
    });
    let mut out: Vec<Candidate> = Vec::new();
    for c in candidates.drain(..) {
        let nested = out.iter().any(|existing| {
            existing.is_directory && is_parent_of_or_equal(&existing.path, &c.path)
        });
        if !nested {
            out.push(c);
        }
    }
    *candidates = out;
}

/// Port of `discoverCandidates` (manual recursion replaces
/// recursive_directory_iterator; symlinked directories are never followed).
pub fn discover_candidates(enabled: &crate::app::EnabledMap) {
    let roots = get_existing_roots();
    let roots_line = roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("; ");
    log_line("INFO", &format!("Search roots: {roots_line}"));

    let mut seen: HashSet<String> = HashSet::new();

    let mut process_path = |p: &Path| {
        // No-follow kind so a junction surface is recorded as its own entry
        // without claiming directory semantics of its TARGET.
        let is_dir = matches!(
            crate::fsutil::path_kind_no_follow(p),
            Ok(crate::fsutil::PathKind::Directory)
        );
        let Some(comp) = match_component_for_path(p, enabled) else {
            return;
        };
        let canon_key = {
            let canon = p.canonicalize().map(|c| path_lower(&c)).unwrap_or_default();
            if canon.is_empty() {
                path_lower(p)
            } else {
                canon
            }
        };
        if seen.insert(canon_key) {
            app::run_mut(|s| {
                s.candidates.push(Candidate {
                    path: p.to_path_buf(),
                    component_key: comp.key.clone(),
                    component_name: comp.display_name.clone(),
                    is_directory: is_dir,
                })
            });
        }
    };

    for root in &roots {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            app::run_mut(|s| s.aborted = true);
            break;
        }
        process_path(root);
        if matches!(
            crate::fsutil::path_kind_no_follow(root),
            Ok(crate::fsutil::PathKind::Directory)
        ) {
            walk(root, enabled, &mut process_path);
        }
    }

    if app::run(|s| s.aborted) {
        log_line("WARN", "Candidate scan aborted early by user request.");
    }

    app::run_mut(|s| collapse_nested_candidates(&mut s.candidates));
    let candidate_count = app::run(|s| s.candidates.len());
    log_line("INFO", &format!("Candidate count: {candidate_count}"));

    // Per-component breakdown so an already-clean system explicitly reports
    // 0 for every area instead of one opaque global count.
    let per_component: Vec<(String, usize)> = crate::components::build_components()
        .iter()
        .map(|c| {
            let n = app::run(|s| {
                s.candidates
                    .iter()
                    .filter(|cd| cd.component_key == c.key)
                    .count()
            });
            (c.key.clone(), n)
        })
        .collect();

    let mut ss = String::from("\n==== Discovery summary by component ====\n");
    let mut line = String::new();
    for (key, n) in &per_component {
        ss.push_str(&format!("{key}: {n}\n"));
        if !line.is_empty() {
            line.push_str(", ");
        }
        line.push_str(&format!("{key}={n}"));
    }
    let log_path = app::run(|s| s.log_path.clone());
    append_utf8_file(&log_path, &ss);
    log_line("INFO", &format!("Discovered per component: {line}"));

    if candidate_count == 0 {
        log_line(
            "INFO",
            "Nothing matched - the system appears already clean (or only disabled optional components were scanned). For what earlier runs removed, see their logs / the previous-run history line.",
        );
    }
}

/// Depth-first traversal with prune support and abort checks. Symlinks are
/// not followed (parity with recursive_directory_iterator defaults).
fn walk(dir: &Path, enabled: &crate::app::EnabledMap, visit: &mut dyn FnMut(&Path)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log_line(
                "WARN",
                &format!("Traversal warning for {}: {}", dir.display(), e),
            );
            return;
        }
    };
    for entry in entries.flatten() {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            app::run_mut(|s| s.aborted = true);
            return;
        }
        let path = entry.path();
        // Reparse entries are visited as candidate surfaces but NEVER
        // descended into (FS-01 destructive boundary).
        let attrs = entry.metadata().map(|m| m.file_attributes()).unwrap_or(0);
        let is_reparse = crate::fsutil::attrs_are_reparse(attrs);
        if !is_reparse && should_prune_traversal(&path, enabled) {
            continue;
        }
        visit(&path);
        let descend = !is_reparse && entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if descend {
            walk(&path, enabled, visit);
        }
    }
}

/// Port of `tallyPreviousLogs`: parses other debloat-*.log files beside the
/// executable. The markers below are a cross-version contract — do not change.
pub fn tally_previous_logs() {
    const RUN_HEADER_MARKER: &str = "run header ====";
    const CANDIDATE_COUNT_MARKER: &str = "Candidate count: ";

    let exe_dir = app::run(|s| s.exe_dir.clone());
    let current = app::run(|s| path_lower(&s.log_path));
    let mut log_files = 0usize;
    let mut runs = 0usize;
    let mut candidates_found: i64 = 0;

    if let Ok(entries) = std::fs::read_dir(&exe_dir) {
        for entry in entries.flatten() {
            let low = to_lower(&entry.file_name().to_string_lossy());
            if !low.starts_with("debloat-") || !ends_with_no_case(&low, ".log") {
                continue;
            }
            if path_lower(&entry.path()) == current {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            log_files += 1;
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                if line.contains(RUN_HEADER_MARKER) {
                    runs += 1;
                }
                if let Some(pos) = line.find(CANDIDATE_COUNT_MARKER) {
                    let tail = line[pos + CANDIDATE_COUNT_MARKER.len()..].trim();
                    if let Ok(v) = trim(tail).parse::<i64>() {
                        candidates_found += v;
                    }
                }
            }
        }
    }

    app::run_mut(|s| {
        s.history_scan_done = true;
        s.history_log_files = log_files as i64;
        s.history_runs = runs as i64;
        s.history_candidates = candidates_found;
    });

    if log_files == 0 {
        log_line(
            "INFO",
            "Previous-run history: no earlier debloat-*.log files beside the executable.",
        );
    } else {
        log_line(
            "INFO",
            &format!(
                "Previous-run history: {log_files} earlier log file(s), {runs} recorded run(s), \
                 {candidates_found} candidate paths discovered across them (see those logs for the exact paths)."
            ),
        );
    }
}

/// Port of `writeCandidatesCsv`: appends the candidates list to the run log
/// instead of a separate CSV, so a full run leaves exactly one file behind.
pub fn write_candidates_to_log() {
    let (count, rows, log_path) = app::run(|s| {
        let rows: Vec<String> = s
            .candidates
            .iter()
            .map(|c| {
                format!(
                    "{},\"{}\",{}",
                    c.component_key,
                    c.path.to_string_lossy(),
                    if c.is_directory { "directory" } else { "file" }
                )
            })
            .collect();
        (s.candidates.len(), rows, s.log_path.clone())
    });

    let mut ss = format!("==== Candidates ({count} total) ====\n");
    ss.push_str("component,path,type\n");
    for r in rows {
        ss.push_str(&r);
        ss.push('\n');
    }
    append_utf8_file(&log_path, &ss);
}
