//! Diagnostics for deletions that cannot finish now: which process holds an
//! undeletable entry (Restart Manager), whether that is an expected
//! NVIDIA-container lock, and which paths Windows already queues as
//! reboot-pending deletions from an earlier run. Read-only: nothing here
//! mutates the system or changes what gets deleted/scheduled — it only
//! shapes log and report wording.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::app;
use crate::logging::log_line;
use crate::matching::{is_nvidia_context_string, is_preserved_container_name};
use crate::util::to_lower;
use crate::winfmt::format_win_error;

/// One entry a deletion attempt could not remove.
pub struct FailedEntry {
    pub path: PathBuf,
    /// Win32 error code when the OS reported one.
    pub code: Option<u32>,
    pub message: String,
}

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_LOCK_VIOLATION: u32 = 33;

/// Restart Manager lookups are diagnostics only; bounded per run so a tree
/// full of locked entries cannot stall the destructive stage.
const MAX_LOCK_QUERIES_PER_RUN: usize = 32;
/// Undeletable entries analyzed and listed individually per candidate.
const MAX_LISTED_ENTRIES: usize = 16;

/// Errors that can mean "another process is using this entry". A loaded
/// DLL image reports ACCESS_DENIED, an open file SHARING_VIOLATION.
fn is_lock_like_error(code: Option<u32>) -> bool {
    matches!(
        code,
        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

/// Pure decision core: a failure counts as an expected NVIDIA-container lock
/// only when at least one holder is known and EVERY holder is an NVIDIA
/// container. Unknown holders never qualify (fail toward WARN).
pub fn holders_are_only_nvidia_containers(holder_names: &[String]) -> bool {
    !holder_names.is_empty() && holder_names.iter().all(|n| is_preserved_container_name(n))
}

/// Pure decision core for the DeletePath severity. INFO only when the
/// leftovers will verifiably be handled (reboot scheduling is on), every
/// failure was analyzed, and each one is an NVIDIA-container lock.
pub fn failure_level(
    schedule_for_reboot: bool,
    all_analyzed: bool,
    container_only: &[bool],
) -> &'static str {
    if schedule_for_reboot
        && all_analyzed
        && !container_only.is_empty()
        && container_only.iter().all(|c| *c)
    {
        "INFO"
    } else {
        "WARN"
    }
}

struct EntryAnalysis {
    /// "NVDisplay.Container.exe PID=123", ...; empty when none known.
    holders: Vec<String>,
    container_only: bool,
    /// Why no holder is listed (lookup failure, budget, no report).
    note: Option<String>,
}

impl EntryAnalysis {
    fn none(note: Option<String>) -> EntryAnalysis {
        EntryAnalysis {
            holders: Vec::new(),
            container_only: false,
            note,
        }
    }

    /// " (held by: ...)" / " (no lock holder reported)" / "".
    fn suffix(&self) -> String {
        if !self.holders.is_empty() {
            format!(" (held by: {})", self.holders.join(", "))
        } else if let Some(note) = &self.note {
            format!(" ({note})")
        } else {
            String::new()
        }
    }
}

/// Path as the user sees it (no `\\?\` extended-length prefix).
pub fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    match s.strip_prefix("\\\\?\\") {
        Some(rest) => rest.to_string(),
        None => s.into_owned(),
    }
}

fn analyze_entry(entry: &FailedEntry, exe_by_pid: &BTreeMap<u32, String>) -> EntryAnalysis {
    if !is_lock_like_error(entry.code) {
        return EntryAnalysis::none(None);
    }
    let shown = display_path(&entry.path);

    // An image mapped by an NVIDIA container (seen by the module inspection,
    // which only examines container processes) cannot be deleted while
    // loaded; Restart Manager would only repeat that.
    let loaded_by = app::run(|s| s.container_loaded_modules.get(&to_lower(&shown)).cloned());
    if let Some(pids) = loaded_by {
        let holders = pids
            .iter()
            .map(|pid| {
                let image = exe_by_pid
                    .get(pid)
                    .map(String::as_str)
                    .unwrap_or("NVIDIA container");
                format!("{image} PID={pid}, module loaded")
            })
            .collect();
        return EntryAnalysis {
            holders,
            container_only: true,
            note: None,
        };
    }

    let within_budget = app::run_mut(|s| {
        if s.lock_queries < MAX_LOCK_QUERIES_PER_RUN {
            s.lock_queries += 1;
            true
        } else {
            false
        }
    });
    if !within_budget {
        return EntryAnalysis::none(Some("lock lookup budget exhausted".to_string()));
    }
    match crate::ffi::query_file_lock_holders(&shown) {
        Ok(list) if list.is_empty() => {
            EntryAnalysis::none(Some("no lock holder reported".to_string()))
        }
        Ok(list) => {
            let mut class_names = Vec::new();
            let mut holders = Vec::new();
            for h in list {
                let image = exe_by_pid.get(&h.pid).cloned();
                // Classify by the live image name; RM's own names are only a
                // fallback when the process vanished from the snapshot.
                class_names.push(
                    image
                        .clone()
                        .unwrap_or_else(|| format!("{} {}", h.app_name, h.service_name)),
                );
                let mut text = format!("{} PID={}", image.unwrap_or(h.app_name), h.pid);
                if !h.service_name.is_empty() {
                    text.push_str(&format!(", service {}", h.service_name));
                }
                holders.push(text);
            }
            EntryAnalysis {
                container_only: holders_are_only_nvidia_containers(&class_names),
                holders,
                note: None,
            }
        }
        Err(code) => EntryAnalysis::none(Some(format!(
            "lock lookup failed: {}",
            format_win_error(code)
        ))),
    }
}

/// Log-ready description of one candidate's failed deletion.
pub struct FailureReport {
    pub level: &'static str,
    /// Raw detail for the JSON action record.
    pub detail: String,
    /// Human wording: `<label> [component]: path (<note>)`.
    pub label: &'static str,
    pub note: String,
    /// Per-entry lines (directory candidates only), logged at `entry_level`:
    /// file-only for expected container locks, WARN otherwise.
    pub entry_lines: Vec<String>,
    pub entry_level: &'static str,
}

const LABEL_DEFERRED: &str = "In use - will be deleted at the next restart";

/// Explain a failed deletion: who holds what, and whether this is the
/// expected case of an NVIDIA container keeping a file in use (INFO, the
/// reboot queue finishes it) or something that needs attention (WARN).
pub fn describe_delete_failure(
    is_directory: bool,
    removed: u64,
    failures: &[FailedEntry],
    schedule_for_reboot: bool,
) -> FailureReport {
    let exe_by_pid: BTreeMap<u32, String> = crate::ffi::enum_running_processes()
        .map(|ps| ps.into_iter().map(|p| (p.pid, p.exe_name)).collect())
        .unwrap_or_default();
    let analyses: Vec<EntryAnalysis> = failures
        .iter()
        .take(MAX_LISTED_ENTRIES)
        .map(|f| analyze_entry(f, &exe_by_pid))
        .collect();
    let flags: Vec<bool> = analyses.iter().map(|a| a.container_only).collect();
    let level = failure_level(
        schedule_for_reboot,
        failures.len() <= MAX_LISTED_ENTRIES,
        &flags,
    );
    let expected = level == "INFO";

    let entry_level = if expected { crate::logging::DETAIL } else { "WARN" };
    if !is_directory {
        let (entry, analysis) = (&failures[0], &analyses[0]);
        let report = if expected {
            let holders = analysis.holders.join("; ");
            FailureReport {
                level,
                detail: format!(
                    "In use by NVIDIA container ({holders}); deferred to reboot [{}]",
                    entry.message
                ),
                label: LABEL_DEFERRED,
                note: format!("held by {holders}"),
                entry_lines: Vec::new(),
                entry_level,
            }
        } else {
            FailureReport {
                level,
                detail: format!("Delete failed: {}{}", entry.message, analysis.suffix()),
                label: "Could not delete",
                note: format!("{}{}", entry.message, analysis.suffix()),
                entry_lines: Vec::new(),
                entry_level,
            }
        };
        return report;
    }

    let n = failures.len();
    let entries = |n: usize| if n == 1 { "entry" } else { "entries" };
    let (detail, label, note) = if expected {
        let holders: BTreeSet<&str> = analyses
            .iter()
            .flat_map(|a| a.holders.iter().map(String::as_str))
            .collect();
        let holders = holders.into_iter().collect::<Vec<_>>().join("; ");
        let removed_now = if removed > 0 {
            format!("{removed} {} removed now, ", entries(removed as usize))
        } else {
            String::new()
        };
        (
            format!(
                "Removed entries={removed}; {n} entr{} in use by NVIDIA container ({holders}) deferred to reboot",
                if n == 1 { "y" } else { "ies" },
            ),
            LABEL_DEFERRED,
            format!("{removed_now}{n} {} held by {holders}", entries(n)),
        )
    } else {
        (
            format!(
                "Delete failed: removed {removed} entries, {n} could not be removed (first: {}: {}{})",
                display_path(&failures[0].path),
                failures[0].message,
                analyses[0].suffix()
            ),
            "Could not fully delete",
            format!(
                "{removed} {} removed, {n} {} could not be removed - listed below",
                entries(removed as usize),
                entries(n)
            ),
        )
    };
    let mut entry_lines: Vec<String> = failures
        .iter()
        .zip(analyses.iter())
        .map(|(f, a)| {
            format!(
                "  - not removed: {} ({}{})",
                display_path(&f.path),
                f.message,
                a.suffix()
            )
        })
        .collect();
    if n > MAX_LISTED_ENTRIES {
        entry_lines.push(format!(
            "  - ... and {} more entries not removed",
            n - MAX_LISTED_ENTRIES
        ));
    }
    FailureReport {
        level,
        detail,
        label,
        note,
        entry_lines,
        entry_level,
    }
}

// ---------------------------------------------------------------------------
// Reboot-pending deletions (PendingFileRenameOperations, read-only)
// ---------------------------------------------------------------------------

const SESSION_MANAGER_KEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager";
const PENDING_VALUES: &[&str] = &[
    "PendingFileRenameOperations",
    "PendingFileRenameOperations2",
];

/// `\??\C:\x` / `\\?\C:\x` -> `c:\x` (lowercase comparison key).
pub fn normalize_pending_path(raw: &str) -> String {
    let s = raw
        .strip_prefix("\\??\\")
        .or_else(|| raw.strip_prefix("\\\\?\\"))
        .unwrap_or(raw);
    to_lower(s)
}

/// Pure parser for the REG_MULTI_SZ (source, target) pairs; a deletion is a
/// pair with an empty target. Malformed data never panics: parsing stops at
/// the first empty source (the list terminator).
pub fn parse_pending_deletions(data: &[u16]) -> Vec<String> {
    let parts: Vec<&[u16]> = data.split(|c| *c == 0).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < parts.len() {
        let (src, dst) = (parts[i], parts[i + 1]);
        if src.is_empty() {
            break;
        }
        if dst.is_empty() {
            out.push(normalize_pending_path(&String::from_utf16_lossy(src)));
        }
        i += 2;
    }
    out
}

/// Snapshot Windows' reboot-pending deletions and say whether an earlier
/// run's NVIDIA deletions are still waiting for a reboot.
pub fn note_pending_reboot_deletions() {
    let mut all: HashSet<String> = HashSet::new();
    let mut read_error: Option<String> = None;
    for value in PENDING_VALUES {
        match crate::ffi::read_hklm_multi_sz(SESSION_MANAGER_KEY, value) {
            Ok(Some(data)) => all.extend(parse_pending_deletions(&data)),
            Ok(None) => {}
            Err(e) => read_error = Some(format!("{value}: {}", format_win_error(e))),
        }
    }
    let nvidia = all.iter().filter(|p| is_nvidia_context_string(p)).count();
    let total = all.len();
    log_line(
        crate::logging::DETAIL,
        &format!("Reboot-pending deletions registered by Windows: {total} ({nvidia} under NVIDIA paths)."),
    );
    let message = if let Some(err) = read_error {
        format!("Waiting for a restart: could not check ({err}).")
    } else if nvidia == 0 {
        "Waiting for a restart: no NVIDIA deletions from earlier runs.".to_string()
    } else {
        format!(
            "Waiting for a restart: {nvidia} NVIDIA item(s) from an earlier run will be deleted at the next restart."
        )
    };
    log_line("INFO", &message);
    app::run_mut(|s| s.pending_reboot_deletes = all);
}

/// After discovery: how many candidates an earlier run already queued.
pub fn note_candidates_already_pending() {
    let (already, total) = app::run(|s| {
        let already = s
            .candidates
            .iter()
            .filter(|c| {
                s.pending_reboot_deletes
                    .contains(&to_lower(&display_path(&c.path)))
            })
            .count();
        (already, s.candidates.len())
    });
    if already > 0 {
        log_line(
            "INFO",
            &format!(
                "{already} of the {total} item(s) found are already queued for deletion at the next restart by an earlier run; restarting completes them."
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn pending_parser_keeps_only_deletions() {
        // delete a, rename b -> !c, delete d, list terminator.
        let data = w("\\??\\C:\\NV\\a.dll\0\0\\??\\C:\\b\0!\\??\\C:\\c\0\\??\\C:\\d\0\0\0");
        let got = parse_pending_deletions(&data);
        assert_eq!(got, vec!["c:\\nv\\a.dll".to_string(), "c:\\d".to_string()]);
    }

    #[test]
    fn pending_parser_survives_malformed_data() {
        assert!(parse_pending_deletions(&[]).is_empty());
        assert!(parse_pending_deletions(&w("\0\0")).is_empty());
        // Source without a target element at the very end: not a deletion pair.
        assert!(parse_pending_deletions(&w("\\??\\C:\\x")).is_empty());
        // Missing final terminator still yields the complete pair.
        assert_eq!(parse_pending_deletions(&w("\\??\\C:\\x\0")), vec!["c:\\x"]);
    }

    #[test]
    fn pending_paths_normalize_to_display_keys() {
        assert_eq!(normalize_pending_path("\\??\\C:\\A\\B"), "c:\\a\\b");
        assert_eq!(normalize_pending_path("\\\\?\\C:\\A"), "c:\\a");
        assert_eq!(normalize_pending_path("C:\\A"), "c:\\a");
        assert_eq!(
            display_path(Path::new("\\\\?\\C:\\Program Files\\x")),
            "C:\\Program Files\\x"
        );
    }

    #[test]
    fn only_nvidia_container_holders_count_as_expected() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(holders_are_only_nvidia_containers(&s(&[
            "NVDisplay.Container.exe"
        ])));
        assert!(holders_are_only_nvidia_containers(&s(&[
            "NVDisplay.Container.exe",
            "NVIDIA Display Container LS NVDisplay.ContainerLocalSystem",
        ])));
        // Any foreign holder, or no holder at all, must stay a warning.
        assert!(!holders_are_only_nvidia_containers(&s(&[
            "NVDisplay.Container.exe",
            "MsMpEng.exe",
        ])));
        assert!(!holders_are_only_nvidia_containers(&s(&["setup.exe"])));
        assert!(!holders_are_only_nvidia_containers(&[]));
    }

    #[test]
    fn failure_level_is_info_only_for_fully_explained_container_locks() {
        assert_eq!(failure_level(true, true, &[true, true]), "INFO");
        // Reboot scheduling off: nothing will finish the job.
        assert_eq!(failure_level(false, true, &[true]), "WARN");
        // One unexplained entry, an unanalyzed tail, or nothing analyzed.
        assert_eq!(failure_level(true, true, &[true, false]), "WARN");
        assert_eq!(failure_level(true, false, &[true]), "WARN");
        assert_eq!(failure_level(true, true, &[]), "WARN");
    }

    #[test]
    fn lock_like_errors() {
        assert!(is_lock_like_error(Some(5)));
        assert!(is_lock_like_error(Some(32)));
        assert!(is_lock_like_error(Some(33)));
        assert!(!is_lock_like_error(Some(2)));
        assert!(!is_lock_like_error(None));
    }
}
