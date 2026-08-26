//! TrustedInstaller scheduled-task relaunch orchestration.
//! Ports of `TiRelaunchResult`, `sweepStaleTiArtifacts`,
//! `effectiveChildSwitches`, `attemptTrustedInstallerRelaunch`.
//! All raw COM lives in `ffi`; this module holds the business logic.

use std::time::Duration;

use crate::app;
use crate::ffi::{self, HrError, TiState};
use crate::logging::log_line;
use crate::sysinfo::{is_admin, is_trusted_installer};
use crate::util::join_command;

#[derive(Default)]
pub struct TiRelaunchResult {
    /// a relaunch flow was actually started
    pub attempted: bool,
    /// parent observed the child's completion (via LastTaskResult)
    pub child_status_seen: bool,
    pub child_succeeded: bool,
    pub child_exit_code: i64,
    pub detail: String,
}

/// Removes leftover artifacts of previous crashed runs: stale status files
/// beside the executable and orphaned scheduled tasks with our name prefix.
/// Without this, a reused PID could make a parent read an ancient "ok" status.
pub fn sweep_stale_ti_artifacts() {
    let exe_dir = app::run(|s| s.exe_dir.clone());
    if let Ok(entries) = std::fs::read_dir(&exe_dir) {
        for entry in entries.flatten() {
            let leaf = entry.file_name().to_string_lossy().into_owned();
            if leaf.starts_with(app::K_TI_TASK_PREFIX)
                && crate::util::ends_with_no_case(&leaf, "-status.json")
                && std::fs::remove_file(entry.path()).is_ok()
            {
                log_line(
                    "INFO",
                    &format!("Removed stale TI status file: {}", entry.path().display()),
                );
            }
        }
    }
}

/// Effective-option flags forwarded to every elevated child (UAC relaunch and
/// TrustedInstaller scheduled-task child): wizard defaults from a bare launch
/// and toggles made in the interactive menu exist only in this process' memory.
/// Reconstructing them from raw argv would lose them and let the elevated
/// child run as a dry-run with default selections.
pub fn effective_child_switches() -> Vec<String> {
    let opts = app::opts(|o| o.clone());
    let mut out = Vec::new();
    out.push(if opts.execute {
        "--execute".into()
    } else {
        "--dry-run".into()
    });
    if opts.kill_lockers {
        out.push("--kill-lockers".into());
    }
    if !opts.preserve_nv_containers {
        out.push("--preserve-nvcontainers=off".into());
    }
    if opts.disable_services {
        out.push("--disable-services".into());
    }
    if opts.delete_services {
        out.push("--delete-services".into());
    }
    if opts.delete_scheduled_tasks {
        out.push("--delete-scheduled-tasks".into());
    }
    if !opts.disable_scheduled_tasks {
        out.push("--no-disable-scheduled-tasks".into());
    }
    if opts.schedule_locked_for_reboot {
        out.push("--schedule-reboot-delete".into());
    }
    if opts.take_ownership {
        out.push("--take-ownership".into());
    }
    if opts.include_ngx {
        out.push("--include-ngx".into());
    }
    if opts.include_hd_audio {
        out.push("--include-hdaudio".into());
    }
    if opts.include_physx {
        out.push("--include-physx".into());
    }
    if opts.include_notebook_optimus {
        out.push("--include-notebook-optimus".into());
    }
    if opts.include_virtual_audio {
        out.push("--include-virtual-audio".into());
    }
    if opts.include_nvwmi {
        out.push("--include-nvwmi".into());
    }
    if opts.include_capture_sdk {
        out.push("--include-capture-sdk".into());
    }
    if opts.allow_admin_fallback {
        out.push("--allow-admin-fallback".into());
    }
    out.push(format!("--ti-wait-seconds={}", opts.ti_wait_seconds));
    app::enabled(|m| {
        for (k, v) in m.iter() {
            out.push(format!("--component={k}:{}", if *v { "on" } else { "off" }));
        }
    });
    out
}

/// Pure decision core of the TI-wait poll loop so the scheduler-state
/// machine is unit-testable (audit finding HANG-02). Polling may observe a
/// task that already finished between two polls ONLY through LastTaskResult;
/// holding back that check until the full start-grace would waste ~60 s on
/// every fast child.
#[derive(Debug, PartialEq, Eq)]
pub enum PollDecision {
    /// Stay in the wait loop.
    KeepWaiting,
    /// Task left Running/Queued and carries a real exit code.
    Finished(i64),
    /// Grace elapsed without the task ever starting (HAS_NOT_RUN/unreadable).
    NeverStarted,
}

pub const SCHED_S_TASK_HAS_NOT_RUN: i64 = 0x41303;

pub fn ti_poll_decision(
    saw_running_before: bool,
    state_running: bool,
    state_queued: bool,
    last_result: Option<i64>,
    waited_secs: i64,
    start_grace_secs: i64,
) -> PollDecision {
    let _ = saw_running_before;
    if state_running {
        return PollDecision::KeepWaiting;
    }
    if state_queued {
        return PollDecision::KeepWaiting;
    }
    // Not Running/Queued anymore: consult LastTaskResult IMMEDIATELY.
    match last_result {
        Some(code) if code != SCHED_S_TASK_HAS_NOT_RUN => PollDecision::Finished(code),
        _ => {
            if waited_secs >= start_grace_secs {
                PollDecision::NeverStarted
            } else {
                // HAS_NOT_RUN within the grace window: keep waiting for the
                // scheduler to launch it.
                PollDecision::KeepWaiting
            }
        }
    }
}

/// Port of `attemptTrustedInstallerRelaunch`. The wait loop polls the task
/// state via COM; the exit code comes from LastTaskResult. No handshake file
/// is used, so a run leaves no temporary artifacts.
pub fn attempt_trusted_installer_relaunch() -> TiRelaunchResult {
    let mut res = TiRelaunchResult {
        child_exit_code: -1,
        ..Default::default()
    };

    let opts = app::opts(|o| o.clone());
    if !opts.execute || opts.ti_child || opts.allow_admin_fallback || is_trusted_installer() {
        return res;
    }
    if !opts.attempt_ti_relaunch {
        return res;
    }
    res.attempted = true;

    // Remove stale status files of crashed previous runs beside the exe
    // (the scheduled-task half runs right after COM connect).
    sweep_stale_ti_artifacts();

    if !is_admin() {
        log_line(
            "ERROR",
            "Administrator context is required to create the TrustedInstaller scheduled task.",
        );
        res.detail = "current process is not elevated".to_string();
        return res;
    }

    log_line(
        "INFO",
        "TrustedInstaller context required for destructive execution. Attempting automatic TI scheduled-task relaunch via COM API.",
    );

    let task_name = format!("{}{}", app::K_TI_TASK_PREFIX, ffi_task_id());

    let mut child_args = effective_child_switches();
    child_args.push("--ti-child".to_string());
    child_args.push("--no-menu".to_string());
    // All stages of a run append to the same single log file.
    child_args.push("--log-file".to_string());
    child_args.push(app::run(|s| s.log_path.to_string_lossy().into_owned()));

    // --- COM: ITaskService --------------------------------------------------
    let session = match ffi::TiSession::connect() {
        Ok(s) => s,
        Err(e) => {
            log_line("ERROR", &format!("{}: 0x{:X}", e.message, e.hr));
            res.detail = e.detail.to_string();
            return res;
        }
    };

    if !session.root_available() {
        log_line("ERROR", "GetFolder(\\) failed: root folder unavailable");
        res.detail = "GetFolder failed".to_string();
        return res;
    }

    // Clean leftovers of crashed previous runs before registering our own task.
    for name in session.sweep_stale_tasks(app::K_TI_TASK_PREFIX) {
        log_line("INFO", &format!("Removed stale TI scheduled task: {name}"));
    }

    // Remove previous instance if present, then register + run ours.
    session.delete_task_if_exists(&task_name);

    let exe_path = app::run(|s| s.exe_path.to_string_lossy().into_owned());
    let work_dir = app::run(|s| s.exe_dir.to_string_lossy().into_owned());
    let registered = match session.register_and_run_ti_child(
        &task_name,
        &exe_path,
        &join_command(&child_args),
        &work_dir,
    ) {
        Ok(t) => t,
        Err(HrError(hr)) => {
            log_line("ERROR", &format!("RegisterTaskDefinition failed: 0x{hr:X}"));
            res.detail = "RegisterTaskDefinition failed".to_string();
            return res;
        }
    };
    log_line("INFO", &format!("Task registered: {task_name}"));
    log_line(
        "INFO",
        "Task launched. Waiting for the SYSTEM worker to finish (exit code via LastTaskResult).",
    );

    // Wait for task completion; the exit code comes from LastTaskResult.
    let mut waited: i64 = 0;
    let start_grace: i64 = 60; // seconds to allow the task to enter Running at all
    let mut saw_running = false;
    let mut timed_out = true;

    while waited < opts.ti_wait_seconds {
        let polled = registered.poll_state();
        let (state_running, state_queued) = match polled {
            Some(TiState::Running) => {
                saw_running = true;
                (true, false)
            }
            Some(TiState::Queued) => (false, true),
            _ => (false, false),
        };
        let last = registered.last_result();

        match ti_poll_decision(
            saw_running,
            state_running,
            state_queued,
            last,
            waited,
            start_grace,
        ) {
            PollDecision::KeepWaiting => {}
            PollDecision::Finished(exit_code) => {
                res.child_status_seen = true;
                res.child_exit_code = exit_code;
                res.child_succeeded = exit_code == 0;
                res.detail = format!("TI child finished: exit=0x{exit_code:X}");
                log_line(
                    if res.child_succeeded { "INFO" } else { "ERROR" },
                    &res.detail,
                );
                timed_out = false;
                break;
            }
            PollDecision::NeverStarted => {
                log_line(
                    "ERROR",
                    &format!(
                        "TI task did not enter Running state within {start_grace}s. Task Scheduler may have refused the S4U/TrustedInstaller principal."
                    ),
                );
                res.detail = "task never started".to_string();
                timed_out = false;
                break;
            }
        }

        if waited > 0 && waited % 15 == 0 {
            log_line(
                "INFO",
                &format!("Still waiting for TI child... ({waited}s)"),
            );
        }
        std::thread::sleep(Duration::from_secs(3));
        waited += 3;
    }

    if timed_out {
        log_line(
            "ERROR",
            &format!(
                "Timed out waiting for TI child after {} seconds.",
                opts.ti_wait_seconds
            ),
        );
        res.detail = "timed out waiting for child".to_string();
    }

    session.finish_task(&registered, &task_name);
    res
}

fn ffi_task_id() -> u32 {
    crate::ffi::current_process_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_completion_between_polls_finishes_immediately() {
        // HANG-02: child started and exited between two polls must be
        // recognized at once via LastTaskResult, well BEFORE the 60 s grace.
        assert_eq!(
            ti_poll_decision(false, false, false, Some(0), 3, 60),
            PollDecision::Finished(0)
        );
        assert_eq!(
            ti_poll_decision(false, false, false, Some(11), 3, 60),
            PollDecision::Finished(11)
        );
    }

    #[test]
    fn has_not_run_keeps_waiting_until_grace_expires() {
        assert_eq!(
            ti_poll_decision(false, false, false, Some(SCHED_S_TASK_HAS_NOT_RUN), 3, 60),
            PollDecision::KeepWaiting,
            "scheduler simply has not launched yet"
        );
        // Unreadable result behaves like HAS_NOT_RUN inside the grace.
        assert_eq!(
            ti_poll_decision(false, false, false, None, 59, 60),
            PollDecision::KeepWaiting
        );
        // ...but past the grace it becomes the never-started diagnosis.
        assert_eq!(
            ti_poll_decision(false, false, false, Some(SCHED_S_TASK_HAS_NOT_RUN), 60, 60),
            PollDecision::NeverStarted
        );
        assert_eq!(
            ti_poll_decision(false, false, false, None, 63, 60),
            PollDecision::NeverStarted
        );
    }

    #[test]
    fn running_and_queued_states_keep_waiting() {
        assert_eq!(
            ti_poll_decision(true, true, false, Some(0xC1), 120, 60),
            PollDecision::KeepWaiting
        );
        assert_eq!(
            ti_poll_decision(false, false, true, None, 10, 60),
            PollDecision::KeepWaiting
        );
    }
}
