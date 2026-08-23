//! Reporting and the main cleanup flow. Ports of `writeReport`,
//! `verifyCandidateRemoval`, `runCleanup`.

use std::collections::BTreeMap;

use crate::actions::{
    handle_scheduled_tasks, handle_services, inspect_nv_container_modules, kill_locker_processes,
};
use crate::app;
use crate::components::build_components;
use crate::discovery::{discover_candidates, tally_previous_logs, write_candidates_to_log};
use crate::logging::{append_utf8_file, log_line};
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
    append_utf8_file(&log_path, &ss);

    let status_summary = by_status
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");
    log_line(
        "INFO",
        &format!("Action summary by status: {status_summary}"),
    );
}

/// Port of `verifyCandidateRemoval`: re-checks every discovered candidate path
/// after processing and appends a "Post-run existence check" section.
pub fn verify_candidate_removal() {
    // Latest DeletePath/ScheduleDelete outcome per candidate path.
    let mut path_outcome: BTreeMap<String, (String, String)> = BTreeMap::new();
    app::run(|s| {
        for a in &s.actions {
            if a.kind == "DeletePath" {
                path_outcome.insert(a.path.clone(), (a.status.clone(), a.detail.clone()));
            } else if a.kind == "ScheduleDelete" {
                path_outcome.insert(
                    a.path.clone(),
                    ("SCHEDULED_REBOOT".to_string(), a.detail.clone()),
                );
            }
        }
    });

    let execute = app::opts(|o| o.execute);
    let candidates = app::run(|s| s.candidates.clone());

    let mut remaining: usize = 0;
    let mut pending_reboot: usize = 0;
    let mut dry_run: usize = 0;
    let mut ss = String::from("\n==== Post-run existence check ====\n");
    for c in &candidates {
        if !c.path.exists() {
            continue; // gone: exactly what we want
        }
        remaining += 1;
        let path_key = c.path.to_string_lossy().into_owned();
        let outcome = path_outcome.get(path_key.as_str());
        let status = outcome.map(|(st, _)| st.as_str()).unwrap_or("");
        let detail = outcome.map(|(_, d)| d.as_str()).unwrap_or("");
        let reason;
        if !execute || status == "DRYRUN" {
            reason = "dry-run: not attempted".to_string();
            dry_run += 1;
        } else if status == "SCHEDULED_REBOOT" {
            reason = "scheduled for deletion at next reboot".to_string();
            pending_reboot += 1;
        } else if status == "SKIP" {
            reason = format!("skipped: {detail}");
        } else if status == "WARN" {
            reason = format!("deletion failed: {detail}");
        } else if status.is_empty() {
            reason = "no action recorded".to_string();
        } else {
            reason = "still present".to_string();
        }
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

    if candidates.is_empty() {
        ss.push_str("No NVIDIA bloat candidates were discovered in this run.\n");
    } else if remaining == 0 {
        ss.push_str(&format!(
            "Result: all {} discovered paths are gone (0 remaining).\n",
            candidates.len()
        ));
    } else {
        let mut summary = format!(
            "Summary: {remaining} of {} discovered paths still exist",
            candidates.len()
        );
        if pending_reboot > 0 {
            summary.push_str(&format!(" ({pending_reboot} pending reboot deletion)"));
        }
        if dry_run > 0 {
            summary.push_str(&format!(" ({dry_run} not attempted in dry-run)"));
        }
        summary.push('.');
        ss.push_str(&summary);
    }
    let log_path = app::run(|s| s.log_path.clone());
    append_utf8_file(&log_path, &ss);

    if candidates.is_empty() {
        log_line(
            "INFO",
            "Post-run check: no NVIDIA bloat candidates discovered; nothing to verify (0 paths remain).",
        );
    } else {
        log_line(
            if remaining == 0 { "INFO" } else { "WARN" },
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

/// Port of `runCleanup`. Returns Err(message) where the C++ threw — the caller
/// maps that onto the FATAL/exit-code-1 path.
pub fn run_cleanup() -> Result<(), String> {
    let run_id = app::run(|s| s.run_id.clone());
    let exe_path = app::run(|s| s.exe_path.to_string_lossy().into_owned());
    let log_path = app::run(|s| s.log_path.to_string_lossy().into_owned());
    let opts = app::opts(|o| o.clone());

    log_line("INFO", "==== NVIDIA post-install debloat run header ====");
    log_line("INFO", &format!("RunId: {run_id}"));
    log_line("INFO", &format!("Exe: {exe_path}"));
    log_line("INFO", &format!("Log: {log_path}"));
    log_line("INFO", &format!("Identity: {}", current_token_account()));
    log_line(
        "INFO",
        &format!("IsAdmin: {}", if is_admin() { "True" } else { "False" }),
    );
    log_line(
        "INFO",
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
        "INFO",
        &format!(
            "Execution mode: {}",
            if opts.execute { "EXECUTE" } else { "DRY RUN" }
        ),
    );
    if !opts.execute {
        log_line(
            "INFO",
            "Dry run does not request TrustedInstaller; it only scans and logs as the current user/admin.",
        );
    }
    if opts.execute && !opts.ti_child && !opts.allow_admin_fallback && !is_trusted_installer() {
        log_line(
            "INFO",
            "Execute mode will attempt TrustedInstaller scheduled-task relaunch before destructive actions.",
        );
    }
    log_line(
        "INFO",
        &format!(
            "Preserve NVIDIA Container processes/services: {}",
            if opts.preserve_nv_containers {
                "True"
            } else {
                "False"
            }
        ),
    );

    let comp_summary = build_components()
        .iter()
        .map(|c| {
            let on = app::enabled(|m| m.get(&c.key).copied().unwrap_or(false));
            format!("{}={}", c.key, if on { "on" } else { "off" })
        })
        .collect::<Vec<_>>()
        .join(", ");
    log_line("INFO", &format!("Components: {comp_summary}"));

    if opts.execute && !opts.ti_child && !is_trusted_installer() && !opts.allow_admin_fallback {
        return Err(
            "Destructive execution requires TrustedInstaller. Relaunch did not occur or did not enter TI context."
                .to_string(),
        );
    }
    if opts.ti_child && !is_trusted_installer() {
        log_line(
            "WARN",
            &format!(
                "TI child running as {} instead of TrustedInstaller. SYSTEM-level privileges should suffice for file operations.",
                current_token_account()
            ),
        );
    }

    let enabled_snapshot = app::enabled_snapshot();

    handle_services(&enabled_snapshot);
    kill_locker_processes();
    handle_scheduled_tasks();
    tally_previous_logs();
    inspect_nv_container_modules(false);
    discover_candidates(&enabled_snapshot);
    write_candidates_to_log();

    let candidate_count = app::run(|s| s.candidates.len());
    for index in 0..candidate_count {
        if app::ABORT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
            app::run_mut(|s| s.aborted = true);
            break;
        }
        crate::deletion::delete_candidate(index);
    }

    inspect_nv_container_modules(true);
    verify_candidate_removal();
    if app::run(|s| s.aborted) {
        log_line(
            "WARN",
            "Run aborted by user request; partial results recorded.",
        );
    }
    write_report();
    log_line("INFO", &format!("Done. Log: {log_path}"));
    if !opts.execute {
        log_line(
            "WARN",
            "Dry run only. Re-run with --execute to make changes.",
        );
    }
    if opts.schedule_locked_for_reboot {
        log_line(
            "WARN",
            "Some locked-file deletions may require reboot. Review report/log.",
        );
    }
    Ok(())
}
