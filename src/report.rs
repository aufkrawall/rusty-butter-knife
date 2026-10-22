//! Reporting and the main cleanup flow. Ports of `writeReport`,
//! `verifyCandidateRemoval`, `runCleanup`.

use std::collections::BTreeMap;

use crate::actions::{
    handle_scheduled_tasks, handle_services, inspect_nv_container_modules, kill_locker_processes,
};
use crate::app;
use crate::components::build_components;
use crate::discovery::{discover_candidates, tally_previous_logs, write_candidates_to_log};
use crate::logging::{append_block_serialized, log_line, DETAIL, STEP};
use crate::sysinfo::{current_token_account, is_admin, is_trusted_installer};
use crate::util::json_escape;

/// Port of `writeReport`: appends the JSON report block to the single run log.
pub fn write_report() {
    let (by_status, by_type) = app::run(|s| {
        let mut by_status: BTreeMap<String, i64> = BTreeMap::new();
        let mut by_type: BTreeMap<String, i64> = BTreeMap::new();
        for a in &s.actions {
            *by_status.entry(a.status.clone()).or_insert(0) += 1;
            *by_type.entry(a.kind.clone()).or_insert(0) += 1;
        }
        (by_status, by_type)
    });

    let snapshot = app::run(|s| {
        (
            s.run_id.clone(),
            s.exe_path.to_string_lossy().into_owned(),
            s.log_path.to_string_lossy().into_owned(),
            s.aborted,
            s.candidates.len(),
            s.post_run_check_done,
            s.paths_remaining_after_run,
            s.paths_pending_reboot,
            s.history_scan_done,
            s.history_log_files,
            s.history_runs,
            s.history_candidates,
            s.actions.clone(),
        )
    });
    let opts = app::opts(|o| o.clone());
    let identity = current_token_account();

    let mut ss = String::from("\n==== Run report (JSON) ====\n");
    ss.push_str("{\n");
    ss.push_str(&format!("  \"runId\": \"{}\",\n", json_escape(&snapshot.0)));
    ss.push_str(&format!(
        "  \"exePath\": \"{}\",\n",
        json_escape(&snapshot.1)
    ));
    ss.push_str(&format!(
        "  \"identity\": \"{}\",\n",
        json_escape(&identity)
    ));
    ss.push_str(&format!("  \"isAdmin\": {},\n", is_admin()));
    ss.push_str(&format!(
        "  \"isTrustedInstaller\": {},\n",
        is_trusted_installer()
    ));
    ss.push_str(&format!("  \"execute\": {},\n", opts.execute));
    ss.push_str(&format!("  \"version\": \"{}\",\n", app::GPD_VERSION));
    ss.push_str(&format!(
        "  \"logPath\": \"{}\",\n",
        json_escape(&snapshot.2)
    ));
    ss.push_str(&format!("  \"aborted\": {},\n", snapshot.3));
    ss.push_str(&format!("  \"candidateCount\": {},\n", snapshot.4));
    if snapshot.5 {
        ss.push_str(&format!(
            "  \"candidatePathsRemainingAfterRun\": {},\n",
            snapshot.6
        ));
        ss.push_str(&format!(
            "  \"candidatePathsPendingRebootDelete\": {},\n",
            snapshot.7
        ));
    }
    if snapshot.8 {
        ss.push_str(&format!(
            "  \"previousLogs\": {{\"files\": {}, \"runs\": {}, \"candidatesDiscoveredTotal\": {}}},\n",
            snapshot.9, snapshot.10, snapshot.11
        ));
    }

    // Components in catalog order.
    ss.push_str("  \"components\": {");
    let comps_state: Vec<(String, bool)> = build_components()
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                app::enabled(|m| m.get(&c.key).copied().unwrap_or(false)),
            )
        })
        .collect();
    for (i, (key, on)) in comps_state.iter().enumerate() {
        if i > 0 {
            ss.push_str(", ");
        }
        ss.push_str(&format!("\"{}\": {}", json_escape(key), on));
    }
    ss.push_str("},\n");

    ss.push_str(&format!(
        "  \"options\": {{\"killLockers\": {}, \"disableServices\": {}, \"deleteServices\": {}, \
         \"disableScheduledTasks\": {}, \"deleteScheduledTasks\": {}, \"scheduleLockedForReboot\": {}, \
         \"takeOwnership\": {}, \"preserveNvContainers\": {}, \"allowAdminFallback\": {}, \"tiChild\": {}}},\n",
        opts.kill_lockers,
        opts.disable_services,
        opts.delete_services,
        opts.disable_scheduled_tasks,
        opts.delete_scheduled_tasks,
        opts.schedule_locked_for_reboot,
        opts.take_ownership,
        opts.preserve_nv_containers,
        opts.allow_admin_fallback,
        opts.ti_child,
    ));

    let map_json = |m: &BTreeMap<String, i64>| -> String {
        m.iter()
            .map(|(k, v)| format!("\"{}\": {}", json_escape(k), v))
            .collect::<Vec<_>>()
            .join(", ")
    };
    ss.push_str(&format!(
        "  \"summaryByStatus\": {{{}}},\n",
        map_json(&by_status)
    ));
    ss.push_str(&format!(
        "  \"summaryByType\": {{{}}},\n",
        map_json(&by_type)
    ));

    ss.push_str("  \"actions\": [\n");
    let actions = &snapshot.12;
    for (i, a) in actions.iter().enumerate() {
        ss.push_str("    {\"type\": \"");
        ss.push_str(&json_escape(&a.kind));
        ss.push_str("\", \"status\": \"");
        ss.push_str(&json_escape(&a.status));
        ss.push_str("\", \"component\": \"");
        ss.push_str(&json_escape(&a.component));
        ss.push_str("\", \"path\": \"");
        ss.push_str(&json_escape(&a.path));
        ss.push_str("\", \"detail\": \"");
        ss.push_str(&json_escape(&a.detail));
        ss.push_str("\"}");
        if i + 1 < actions.len() {
            ss.push(',');
        }
        ss.push('\n');
    }
    ss.push_str("  ]\n");
    ss.push_str("}\n");

    let log_path = app::run(|s| s.log_path.clone());
    append_block_serialized(&log_path, &ss); // JSON report block: serialized

    let status_summary = by_status
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");
    log_line(
        DETAIL,
        &format!("Action summary by status: {status_summary}"),
    );
}

/// Port of `verifyCandidateRemoval`: re-checks every discovered candidate path
/// after processing and appends a "Post-run existence check" section.
/// Pure decision core for one candidate's post-run report row. `scheduled`
/// is true only when MoveFileExW verifiably succeeded for that exact path
/// (REPORT-01); verification I/O failures are their own outcome so they
/// cannot be reported as success (REPORT-02).
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyOutcome {
    DryRun,
    ScheduledReboot,
    Skipped(String),
    Failed(String),
    NoAction,
    StillPresent,
}

pub fn verify_outcome_for(
    execute: bool,
    status: &str,
    detail: &str,
    scheduled: bool,
) -> VerifyOutcome {
    if !execute || status == "DRYRUN" {
        return VerifyOutcome::DryRun;
    }
    if scheduled && status != "WARN" && status != "SKIP" {
        return VerifyOutcome::ScheduledReboot;
    }
    match status {
        "SKIP" => VerifyOutcome::Skipped(detail.to_string()),
        "WARN" | "FATAL" => VerifyOutcome::Failed(detail.to_string()),
        "" => VerifyOutcome::NoAction,
        _ => VerifyOutcome::StillPresent,
    }
}

pub fn verify_candidate_removal() {
    // Latest DeletePath/ScheduleDelete outcome per candidate path.
    let mut path_outcome: BTreeMap<String, (String, String)> = BTreeMap::new();
    app::run(|s| {
        for a in &s.actions {
            if a.kind == "DeletePath" || a.kind == "ScheduleDelete" {
                path_outcome.insert(a.path.clone(), (a.status.clone(), a.detail.clone()));
            }
        }
    });

    let execute = app::opts(|o| o.execute);
    let candidates = app::run(|s| s.candidates.clone());
    let reboot_scheduled = app::run(|s| s.reboot_scheduled_paths.clone());

    let mut remaining: usize = 0;
    let mut pending_reboot: usize = 0;
    let mut dry_run: usize = 0;
    let mut errors: usize = 0;
    let mut ss = String::from("\n==== Post-run existence check ====\n");
    for c in &candidates {
        // Tri-state no-follow probe: metadata failure must not pass as
        // "successfully deleted" (REPORT-02).
        let stat = crate::fsutil::path_kind_no_follow(&c.path);
        if matches!(stat, Ok(crate::fsutil::PathKind::Missing)) {
            continue; // gone: exactly what we want
        }
        remaining += 1;
        let path_key = c.path.to_string_lossy().into_owned();
        let outcome = path_outcome.get(path_key.as_str());
        let status = outcome.map(|(st, _)| st.as_str()).unwrap_or("");
        let detail = outcome.map(|(_, d)| d.as_str()).unwrap_or("");
        let scheduled = reboot_scheduled.contains(&crate::util::to_lower(&path_key));
        let reason = if let Err(e) = &stat {
            errors += 1;
            format!("verification error: {e}")
        } else {
            match verify_outcome_for(execute, status, detail, scheduled) {
                VerifyOutcome::DryRun => {
                    dry_run += 1;
                    "dry-run: not attempted".to_string()
                }
                VerifyOutcome::ScheduledReboot => {
                    pending_reboot += 1;
                    "scheduled for deletion at next reboot".to_string()
                }
                VerifyOutcome::Skipped(d) => format!("skipped: {d}"),
                VerifyOutcome::Failed(d) => format!("deletion failed: {d}"),
                VerifyOutcome::NoAction => "no action recorded".to_string(),
                VerifyOutcome::StillPresent => "still present".to_string(),
            }
        };
        ss.push_str(&format!(
            "STILL EXISTS [{}] {}\n",
            reason,
            c.path.to_string_lossy()
        ));
    }

    app::run_mut(|s| {
        s.post_run_check_done = true;
        s.paths_remaining_after_run = remaining as i64;
        s.paths_pending_reboot = pending_reboot as i64;
    });

    ss.push_str(&post_run_summary_line(
        candidates.len(),
        remaining,
        pending_reboot,
        errors,
        dry_run,
    ));
    let log_path = app::run(|s| s.log_path.clone());
    append_block_serialized(&log_path, &ss);

    // The human verdict is the closing "Result" section; this stays in the
    // file for tools and diffing against older logs.
    if candidates.is_empty() {
        log_line(
            DETAIL,
            "Post-run check: no NVIDIA bloat candidates discovered; nothing to verify (0 paths remain).",
        );
    } else {
        log_line(
            DETAIL,
            &format!(
                "Post-run check: {}/{} candidate paths removed{}",
                candidates.len() - remaining,
                candidates.len(),
                if remaining > 0 {
                    format!("; {remaining} still exist (details above)")
                } else {
                    String::new()
                }
            ),
        );
    }
}

/// Closing line of the post-run existence check. Always newline-terminated:
/// the next timestamped log line used to be glued onto it.
pub fn post_run_summary_line(
    total: usize,
    remaining: usize,
    pending_reboot: usize,
    errors: usize,
    dry_run: usize,
) -> String {
    if total == 0 {
        return "No NVIDIA bloat candidates were discovered in this run.\n".to_string();
    }
    if remaining == 0 {
        return format!("Result: all {total} discovered paths are gone (0 remaining).\n");
    }
    let mut summary = format!("Summary: {remaining} of {total} discovered paths still exist");
    if pending_reboot > 0 {
        summary.push_str(&format!(" ({pending_reboot} pending reboot deletion)"));
    }
    if errors > 0 {
        summary.push_str(&format!(
            " ({errors} existence checks failed with I/O errors)"
        ));
    }
    if dry_run > 0 {
        summary.push_str(&format!(" ({dry_run} not attempted in dry-run)"));
    }
    summary.push_str(".\n");
    summary
}

/// Port of `runCleanup`. Returns Err(message) where the C++ threw — the caller
/// maps that onto the FATAL/exit-code-1 path.
pub fn run_cleanup() -> Result<(), String> {
    // Fail closed BEFORE any destructive stage: an unwritable audit log must
    // abort the run (audit logging finding).
    let probe_path = app::run(|s| s.log_path.clone());
    crate::ffi::ensure_log_writable(&probe_path)?;
    let run_id = app::run(|s| s.run_id.clone());
    let exe_path = app::run(|s| s.exe_path.to_string_lossy().into_owned());
    let log_path = app::run(|s| s.log_path.to_string_lossy().into_owned());
    let opts = app::opts(|o| o.clone());

    // Machine-oriented header: file only. The marker line below is a
    // cross-version contract (tally_previous_logs counts runs by it).
    let identity = current_token_account();
    let admin = is_admin();
    log_line(DETAIL, "==== NVIDIA post-install debloat run header ====");
    log_line(DETAIL, &format!("RunId: {run_id}"));
    log_line(DETAIL, &format!("Exe: {exe_path}"));
    log_line(DETAIL, &format!("Log: {log_path}"));
    log_line(DETAIL, &format!("Identity: {identity}"));
    log_line(
        DETAIL,
        &format!("IsAdmin: {}", if admin { "True" } else { "False" }),
    );
    log_line(
        DETAIL,
        &format!(
            "IsTrustedInstaller: {}",
            if is_trusted_installer() {
                "True"
            } else {
                "False"
            }
        ),
    );
    log_line(
        DETAIL,
        &format!(
            "Execution mode: {}",
            if opts.execute { "EXECUTE" } else { "DRY RUN" }
        ),
    );
    log_line(
        DETAIL,
        &format!(
            "Preserve NVIDIA Container processes/services: {}",
            if opts.preserve_nv_containers {
                "True"
            } else {
                "False"
            }
        ),
    );
    let enabled_snapshot = app::enabled_snapshot();
    let comp_state: Vec<(&str, bool)> = build_components()
        .iter()
        .map(|c| (c.key.as_str(), enabled_snapshot.get(&c.key).copied().unwrap_or(false)))
        .collect();
    let comp_summary = comp_state
        .iter()
        .map(|(k, on)| format!("{k}={}", if *on { "on" } else { "off" }))
        .collect::<Vec<_>>()
        .join(", ");
    log_line(DETAIL, &format!("Components: {comp_summary}"));

    // Human header.
    log_line(
        STEP,
        &format!(
            "Rusty Butter Knife {} - {}",
            app::RBK_VERSION,
            if opts.execute {
                "EXECUTE: changes will be made"
            } else {
                "DRY RUN: nothing will be changed"
            }
        ),
    );

    if opts.execute && !opts.ti_child && !is_trusted_installer() && !opts.allow_admin_fallback {
        return Err(
            "Destructive execution requires TrustedInstaller. Relaunch did not occur or did not enter TI context."
                .to_string(),
        );
    }
    if opts.ti_child && !is_trusted_installer() {
        let (level, msg) =
            worker_identity_message(crate::sysinfo::is_local_system(), &identity);
        log_line(level, &msg);
    } else {
        log_line(
            "INFO",
            &format!(
                "Running as {identity} ({}).",
                if admin {
                    "administrator"
                } else {
                    "no administrator rights"
                }
            ),
        );
    }
    let (clean, keep): (Vec<&str>, Vec<&str>) = {
        let clean = comp_state.iter().filter(|(_, on)| *on).map(|(k, _)| *k).collect();
        let keep = comp_state.iter().filter(|(_, on)| !*on).map(|(k, _)| *k).collect();
        (clean, keep)
    };
    log_line("INFO", &format!("Will clean: {}", list_or_none(&clean)));
    log_line("INFO", &format!("Will keep:  {}", list_or_none(&keep)));
    if opts.preserve_nv_containers {
        log_line("INFO", "The NVIDIA Display Container is kept running.");
    }

    log_line(STEP, "Services, processes and scheduled tasks");
    handle_services(&enabled_snapshot);
    kill_locker_processes(&enabled_snapshot);
    handle_scheduled_tasks(&enabled_snapshot);

    log_line(STEP, "Looking for NVIDIA bloat");
    tally_previous_logs();
    crate::lockinfo::note_pending_reboot_deletions();
    inspect_nv_container_modules(false, &enabled_snapshot);
    discover_candidates(&enabled_snapshot);
    write_candidates_to_log();
    crate::lockinfo::note_candidates_already_pending();

    let candidate_count = app::run(|s| s.candidates.len());
    if candidate_count > 0 {
        log_line(
            STEP,
            if opts.execute {
                "Removing"
            } else {
                "What would be removed"
            },
        );
    }
    for index in 0..candidate_count {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            app::run_mut(|s| s.aborted = true);
            break;
        }
        crate::deletion::delete_candidate(index);
    }

    // Re-inspection only explains deletions that were actually attempted.
    if opts.execute && candidate_count > 0 {
        inspect_nv_container_modules(true, &enabled_snapshot);
    }
    verify_candidate_removal();
    if app::run(|s| s.aborted) {
        log_line(
            DETAIL,
            "Run aborted by user request; partial results recorded.",
        );
    }
    write_report();

    let facts = app::run(|s| ResultFacts {
        execute: opts.execute,
        aborted: s.aborted,
        found: s.candidates.len(),
        remaining: s.paths_remaining_after_run.max(0) as usize,
        pending_reboot: s.paths_pending_reboot.max(0) as usize,
        reboot_entries: s.reboot_scheduled_paths.len(),
        problems: crate::logging::problems_logged(),
    });
    log_line(STEP, "Result");
    for (level, line) in result_lines(&facts) {
        log_line(level, &line);
    }
    log_line("INFO", &format!("Full log: {log_path}"));
    Ok(())
}

fn list_or_none(items: &[&str]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

/// Inputs of the closing plain-language summary.
pub struct ResultFacts {
    pub execute: bool,
    pub aborted: bool,
    /// Candidate paths found.
    pub found: usize,
    /// Candidate paths still on disk after the run.
    pub remaining: usize,
    /// Of `remaining`: verifiably queued for deletion at the next restart.
    pub pending_reboot: usize,
    /// File-system entries this run queued for deletion at restart.
    pub reboot_entries: usize,
    /// WARN/ERROR/FATAL lines this process logged before the summary.
    pub problems: usize,
}

/// Pure core of the closing "Result" section: what happened and what the
/// user has to do, in plain words.
pub fn result_lines(f: &ResultFacts) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    if f.aborted {
        out.push((
            "WARN",
            "Stopped early (Ctrl+C); only part of the work was done.".to_string(),
        ));
    }
    let plural = |n: usize| if n == 1 { "item" } else { "items" };
    if f.found == 0 {
        out.push((
            "INFO",
            "Nothing to remove - the selected components are already clean.".to_string(),
        ));
    } else if !f.execute {
        out.push((
            "INFO",
            format!(
                "Dry run: {} {} would be removed. Nothing was changed; run with --execute to remove {}.",
                f.found,
                plural(f.found),
                if f.found == 1 { "it" } else { "them" }
            ),
        ));
    } else {
        let removed = f.found.saturating_sub(f.remaining);
        let failed = f.remaining.saturating_sub(f.pending_reboot);
        out.push((
            "INFO",
            if f.remaining == 0 {
                format!("Removed all {} {} found.", f.found, plural(f.found))
            } else {
                format!("Removed {removed} of {} {} now.", f.found, plural(f.found))
            },
        ));
        if f.pending_reboot > 0 {
            out.push((
                "WARN",
                format!(
                    "{} {} {} still in use and will be deleted when Windows restarts. Please restart to finish.",
                    f.pending_reboot,
                    plural(f.pending_reboot),
                    if f.pending_reboot == 1 { "is" } else { "are" }
                ),
            ));
        } else if f.reboot_entries > 0 {
            out.push((
                "WARN",
                "Some leftover parts are queued for deletion when Windows restarts. Please restart to finish."
                    .to_string(),
            ));
        }
        if failed > 0 {
            out.push((
                "WARN",
                format!(
                    "{failed} {} could not be removed - see the WARNING lines above.",
                    plural(failed)
                ),
            ));
        }
    }
    if f.problems > 0 {
        out.push((
            "INFO",
            format!(
                "{} warning(s) were reported during this run (marked WARNING above).",
                f.problems
            ),
        ));
    }
    out
}

/// Identity line of the scheduled-task worker. Task Scheduler does not grant
/// the requested TrustedInstaller principal and runs the task as SYSTEM;
/// that is the normal, sufficient case (INFO). Any other identity is
/// unexpected and stays a warning.
pub fn worker_identity_message(is_local_system: bool, account: &str) -> (&'static str, String) {
    if is_local_system {
        (
            "INFO",
            format!(
                "Running as {account} in the privileged worker (Windows grants SYSTEM instead of the requested TrustedInstaller; that is enough)."
            ),
        )
    } else {
        (
            "WARN",
            format!(
                "Worker running as {account}, neither TrustedInstaller nor LocalSystem; protected NVIDIA files may be denied."
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reboot_schedule_is_not_reported_as_pending() {
        // A WARN ScheduleDelete record with NO successful-set entry must NOT
        // become SCHEDULED_REBOOT (REPORT-01).
        assert_eq!(
            verify_outcome_for(true, "WARN", "0x80070005 access denied", false),
            VerifyOutcome::Failed("0x80070005 access denied".to_string())
        );
    }

    #[test]
    fn successful_schedule_maps_to_pending_reboot() {
        assert_eq!(
            verify_outcome_for(true, "INFO", "Pending delete at reboot", true),
            VerifyOutcome::ScheduledReboot
        );
        // ...and a stale action record never overrides a real failure:
        assert_eq!(
            verify_outcome_for(true, "WARN", "MoveFileExW failed", true),
            VerifyOutcome::Failed("MoveFileExW failed".to_string())
        );
    }

    #[test]
    fn post_run_summary_always_ends_its_line() {
        // Regression: "...pending reboot deletion).[ts] [WARN] Post-run check".
        for line in [
            post_run_summary_line(0, 0, 0, 0, 0),
            post_run_summary_line(8, 0, 0, 0, 0),
            post_run_summary_line(8, 6, 6, 0, 0),
            post_run_summary_line(8, 6, 0, 1, 2),
        ] {
            assert!(line.ends_with(".\n"), "{line:?}");
        }
        assert_eq!(
            post_run_summary_line(8, 6, 6, 0, 0),
            "Summary: 6 of 8 discovered paths still exist (6 pending reboot deletion).\n"
        );
    }

    fn facts(execute: bool, found: usize, remaining: usize, pending: usize) -> ResultFacts {
        ResultFacts {
            execute,
            aborted: false,
            found,
            remaining,
            pending_reboot: pending,
            reboot_entries: pending,
            problems: 0,
        }
    }

    fn texts(lines: &[(&'static str, String)]) -> String {
        lines
            .iter()
            .map(|(l, t)| format!("{l}:{t}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn result_for_clean_system_has_no_restart_or_warning() {
        // Regression: the 0-candidate run ended with a reboot warning.
        let lines = result_lines(&facts(true, 0, 0, 0));
        assert_eq!(lines.len(), 1, "{}", texts(&lines));
        assert!(lines[0].1.contains("already clean"));
        assert!(!texts(&lines).contains("restart"));
    }

    #[test]
    fn result_asks_for_restart_only_when_items_are_queued() {
        // The 2026-10-22 case: 8 found, 6 in use and queued.
        let lines = result_lines(&facts(true, 8, 6, 6));
        let all = texts(&lines);
        assert!(all.contains("INFO:Removed 2 of 8 items now."), "{all}");
        assert!(all.contains("WARN:6 items are still in use"), "{all}");
        assert!(!all.contains("could not be removed"), "{all}");
        let done = texts(&result_lines(&facts(true, 7, 0, 0)));
        assert!(done.contains("Removed all 7 items found."), "{done}");
        assert!(!done.contains("restart"), "{done}");
    }

    #[test]
    fn result_reports_failures_dry_runs_and_aborts() {
        let failed = texts(&result_lines(&facts(true, 3, 2, 1)));
        assert!(failed.contains("1 item could not be removed"), "{failed}");
        let dry = texts(&result_lines(&facts(false, 3, 3, 0)));
        assert!(dry.contains("Dry run: 3 items would be removed"), "{dry}");
        let mut aborted = facts(true, 3, 1, 0);
        aborted.aborted = true;
        aborted.problems = 2;
        let all = texts(&result_lines(&aborted));
        assert!(all.starts_with("WARN:Stopped early"), "{all}");
        assert!(all.contains("2 warning(s)"), "{all}");
    }

    #[test]
    fn worker_identity_is_info_only_for_local_system() {
        assert_eq!(
            worker_identity_message(true, "NT-AUTORITÄT\\SYSTEM").0,
            "INFO"
        );
        assert_eq!(worker_identity_message(false, "PC\\admin").0, "WARN");
    }

    #[test]
    fn dry_run_and_skip_and_noaction_rows() {
        assert_eq!(
            verify_outcome_for(false, "DRYRUN", "", false),
            VerifyOutcome::DryRun
        );
        assert_eq!(
            verify_outcome_for(true, "SKIP", "component deselected", false),
            VerifyOutcome::Skipped("component deselected".to_string())
        );
        assert_eq!(
            verify_outcome_for(true, "", "", false),
            VerifyOutcome::NoAction
        );
        assert_eq!(
            verify_outcome_for(true, "INFO", "Deleted file", false),
            VerifyOutcome::StillPresent
        );
    }
}
