//! Actions on the live system: NVIDIA-container module inspection, locker
//! process termination, bloat service handling, and scheduled-task handling
//! via schtasks.exe. Ports of `inspectNvContainerModules`,
//! `killLockerProcesses`, `handleServices`, `handleScheduledTasks`.

use crate::app;
use crate::ffi;
use crate::ffi_process;
use crate::logging::{add_action, log_action, log_line, DETAIL};
use crate::matching::{
    is_preserved_container_name, match_component_for_path, module_is_telemetry_or_updater,
    process_action_decision, service_action_decision, task_action_decision, ActionDecision,
};
use crate::procs::run_process_capture;
use crate::sysinfo::system_dir_file;
use crate::util::{join_command, parse_csv_line, to_lower, trim};
use crate::winfmt::format_win_error;

/// Pure wording core for one loaded telemetry/update module (`leaf` = file
/// name). `targeted`: the current component selection deletes this path;
/// `scheduled`: its reboot-time deletion verifiably succeeded in this run.
/// None = stay quiet (a kept module was already reported on first sighting).
pub fn loaded_module_message(
    post_delete_phase: bool,
    first_sighting: bool,
    targeted: bool,
    scheduled: bool,
    leaf: &str,
    pid: u32,
) -> Option<(&'static str, String)> {
    if post_delete_phase && scheduled {
        return Some((
            DETAIL,
            format!("{leaf} is still loaded (PID {pid}); it is queued for deletion at the next restart."),
        ));
    }
    match (first_sighting, targeted) {
        (true, true) => Some((
            "INFO",
            format!(
                "{leaf} is loaded by the NVIDIA container (PID {pid}); it can only be deleted after a restart."
            ),
        )),
        (true, false) => Some((
            "INFO",
            format!("{leaf} is loaded by the NVIDIA container (PID {pid}) and kept (its component is off)."),
        )),
        (false, true) if post_delete_phase => Some((
            "INFO",
            format!(
                "{leaf} is still loaded (PID {pid}) and was not queued for deletion; see the result below."
            ),
        )),
        _ => None,
    }
}

/// Port of `inspectNvContainerModules` (runs twice per cleanup; dedup sets
/// live in RunState so sightings are not double-reported across phases).
/// Every loaded telemetry/update module is classified against the component
/// selection, so a module the user keeps is not announced as a pending
/// reboot deletion.
pub fn inspect_nv_container_modules(post_delete_phase: bool, enabled: &crate::app::EnabledMap) {
    log_line(
        DETAIL,
        if post_delete_phase {
            "Re-inspecting NVIDIA Container processes after deletion."
        } else {
            "Inspecting NVIDIA Container processes and loaded telemetry/profile-update modules."
        },
    );

    let processes = match ffi::enum_running_processes() {
        Ok(p) => p,
        Err(e) => {
            log_line(
                "WARN",
                &format!(
                    "CreateToolhelp32Snapshot(process) failed: {}",
                    format_win_error(e)
                ),
            );
            return;
        }
    };

    let mut container_pids: Vec<u32> = Vec::new();
    for proc in processes {
        if !is_preserved_container_name(&proc.exe_name) {
            continue;
        }
        container_pids.push(proc.pid);
        let pid_str = proc.pid.to_string();
        let container_key = format!("{}|{}", pid_str, proc.exe_name);
        let first_container_sighting =
            app::run_mut(|s| s.reported_containers.insert(container_key));
        if first_container_sighting {
            log_line(
                DETAIL,
                &format!("Container running: PID={} Name={}", pid_str, proc.exe_name),
            );
            add_action(
                "ContainerProcess",
                "Running",
                "NvContainer",
                &proc.exe_name,
                &format!("PID={pid_str}"),
            );
        }

        let modules = match ffi::enum_process_modules(proc.pid) {
            Ok(m) => m,
            Err(e) => {
                const ERROR_ACCESS_DENIED: u32 = 5;
                if e == ERROR_ACCESS_DENIED && !crate::sysinfo::is_admin() {
                    // Expected without elevation (SYSTEM-owned service process).
                    log_line(
                        "INFO",
                        &format!(
                            "Cannot look inside the NVIDIA container (PID {}) without administrator rights; this check is skipped.",
                            proc.pid
                        ),
                    );
                } else {
                    log_line(
                        "WARN",
                        &format!(
                            "Could not inspect the NVIDIA container (PID {}): {}",
                            proc.pid,
                            format_win_error(e)
                        ),
                    );
                }
                continue;
            }
        };
        for path in modules {
            if !module_is_telemetry_or_updater(&path) {
                continue;
            }
            let path_key = to_lower(&path);
            let module_key = format!("{}|{}", pid_str, path);
            let first_sighting = app::run_mut(|s| {
                let pids = s.container_loaded_modules.entry(path_key.clone()).or_default();
                if !pids.contains(&proc.pid) {
                    pids.push(proc.pid);
                }
                s.reported_modules.insert(module_key)
            });
            let targeted = match_component_for_path(std::path::Path::new(&path), enabled).is_some();
            let scheduled = app::run(|s| s.reboot_scheduled_paths.contains(&path_key));
            if first_sighting {
                add_action(
                    "LoadedTelemetryModule",
                    "Loaded",
                    "NvContainer",
                    &path,
                    &format!("PID={pid_str}"),
                );
                log_line(
                    DETAIL,
                    &format!("Loaded module: PID={pid_str} Module={path}"),
                );
            }
            let leaf = std::path::Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            if let Some((level, msg)) = loaded_module_message(
                post_delete_phase,
                first_sighting,
                targeted,
                scheduled,
                &leaf,
                proc.pid,
            ) {
                log_line(level, &msg);
            }
        }
    }
    if !post_delete_phase {
        log_line("INFO", &container_summary(&container_pids, app::opts(|o| o.preserve_nv_containers)));
    }
}

/// One console line about the NVIDIA container processes.
pub fn container_summary(pids: &[u32], preserved: bool) -> String {
    if pids.is_empty() {
        return "NVIDIA Display Container: not running.".to_string();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "NVIDIA Display Container: running (PID {list}){}.",
        if preserved { ", kept" } else { "" }
    )
}

/// Stop only explicit historical NVIDIA bloat executables, and verify the
/// live process image through the same handle used for termination. This
/// prevents generic basename false positives and PID-reuse races.
pub fn kill_locker_processes(enabled: &crate::app::EnabledMap) {
    let (kill_lockers, execute, preserve_containers) =
        app::opts(|o| (o.kill_lockers, o.execute, o.preserve_nv_containers));
    if !kill_lockers {
        log_line(
            "INFO",
            "Processes: not stopped (--kill-lockers is not set).",
        );
        return;
    }
    let processes = match ffi::enum_running_processes() {
        Ok(p) => p,
        Err(e) => {
            log_line(
                "WARN",
                &format!("Process snapshot failed: {}", format_win_error(e)),
            );
            return;
        }
    };

    struct Target {
        handle: ffi_process::VerifiedTerminateHandle,
        exe_name: String,
        component_key: String,
    }
    let mut targets: Vec<Target> = Vec::new();
    let (mut known_running, mut verified, mut deselected) = (0usize, 0usize, 0usize);

    for proc in processes {
        if !ffi_process::process_name_is_explicitly_killable(&proc.exe_name) {
            continue;
        }
        known_running += 1;
        let component = match process_action_decision(&proc.exe_name, enabled, preserve_containers)
        {
            ActionDecision::Allowed(c) => c,
            ActionDecision::ComponentDisabled(c) => {
                deselected += 1;
                log_action(
                    "KillProcess",
                    "SKIP",
                    &c.key,
                    &proc.exe_name,
                    &format!("PID={} component deselected", proc.pid),
                );
                continue;
            }
            ActionDecision::NotMatched => continue,
        };

        if !execute {
            match ffi_process::verify_process_for_dry_run(proc.pid, &proc.exe_name) {
                Ok(image) => {
                    verified += 1;
                    log_action(
                        "KillProcess",
                        "DRYRUN",
                        &component.key,
                        &proc.exe_name,
                        &format!("PID={} verified image={image}", proc.pid),
                    );
                }
                Err(detail) => log_action(
                    "KillProcess",
                    "SKIP",
                    &component.key,
                    &proc.exe_name,
                    &format!("PID={} identity verification failed: {detail}", proc.pid),
                ),
            }
            continue;
        }

        let (handle, image) = match ffi_process::VerifiedTerminateHandle::open(
            proc.pid,
            &proc.exe_name,
        ) {
            Ok(v) => v,
            Err(detail) => {
                log_action(
                    "KillProcess",
                    "SKIP",
                    &component.key,
                    &proc.exe_name,
                    &format!("PID={} identity verification failed: {detail}", proc.pid),
                );
                continue;
            }
        };
        log_line(
            "INFO",
            &format!("Verified process target: PID={} Image={image}", proc.pid),
        );
        verified += 1;
        targets.push(Target {
            handle,
            exe_name: proc.exe_name.clone(),
            component_key: component.key.clone(),
        });
    }

    log_line(
        "INFO",
        &if known_running == 0 {
            "Processes: no NVIDIA bloat process is running.".to_string()
        } else {
            format!(
                "Processes: {known_running} NVIDIA bloat process(es) running: {verified} {} stopped, \
                 {deselected} left alone (component off).",
                if execute { "will be" } else { "would be" }
            )
        },
    );

    if !execute {
        return;
    }

    for t in &targets {
        let ok = t.handle.terminate(0);
        log_action(
            "KillProcess",
            if ok { "INFO" } else { "WARN" },
            &t.component_key,
            &t.exe_name,
            &if ok {
                "Terminate requested".to_string()
            } else {
                format_win_error(ffi::current_last_error())
            },
        );
    }

    const CONVERGENCE_MS: u32 = 3000;
    let start = std::time::Instant::now();
    for t in &targets {
        let elapsed = start.elapsed().as_millis() as u32;
        if elapsed >= CONVERGENCE_MS {
            break;
        }
        if !t.handle.wait_ms(CONVERGENCE_MS - elapsed) {
            log_action(
                "KillProcess",
                "WARN",
                &t.component_key,
                &t.exe_name,
                "Still exiting after shared convergence window",
            );
        }
    }
}

/// Port of `handleServices`. Every matched service is classified against the
/// component selection first (SAFETY-01); unknown-classification targets fail
/// closed (never mutated). A requested stop is awaited within a bounded
/// convergence window so subsequent stages do not race an exiting service.
pub fn handle_services(enabled: &crate::app::EnabledMap) {
    let opts = app::opts(|o| {
        (
            o.kill_lockers,
            o.disable_services,
            o.delete_services,
            o.execute,
        )
    });
    let (kill_lockers, disable_services, delete_services, execute) = opts;

    if !kill_lockers && !disable_services && !delete_services {
        log_line(
            "INFO",
            "Services: not checked (no stop/disable/delete option is set).",
        );
        return;
    }

    let scm = match ffi::open_service_control_manager() {
        Ok(s) => s,
        Err(e) => {
            log_line(
                "WARN",
                &format!("OpenSCManager failed: {}", format_win_error(e)),
            );
            return;
        }
    };

    let services = match ffi::enumerate_win32_services() {
        Ok(s) => s,
        Err(e) => {
            log_line(
                "WARN",
                &format!("EnumServicesStatusEx failed: {}", format_win_error(e)),
            );
            return;
        }
    };

    let mut counts = StageCounts {
        scanned: services.len(),
        ..StageCounts::default()
    };
    for svc in services {
        let full = format!("{} ({})", svc.name, svc.display);
        if !ffi_process::has_strong_nvidia_named_target_context(&full) {
            continue;
        }
        counts.nvidia += 1;
        let preserve = app::opts(|o| o.preserve_nv_containers);
        if preserve && is_preserved_container_name(&full) {
            counts.preserved += 1;
        }
        let component = match service_action_decision(&svc.name, &svc.display, enabled, preserve) {
            ActionDecision::Allowed(c) => {
                counts.targeted += 1;
                c
            }
            ActionDecision::ComponentDisabled(c) => {
                counts.deselected += 1;
                log_action(
                    "Service",
                    "SKIP",
                    &c.key,
                    &full,
                    "component deselected",
                );
                continue;
            }
            ActionDecision::NotMatched => continue,
        };
        if !execute {
            if kill_lockers {
                log_action("StopService", "DRYRUN", &component.key, &full, "");
            }
            if disable_services {
                log_action("DisableService", "DRYRUN", &component.key, &full, "");
            }
            if delete_services {
                log_action("DeleteService", "DRYRUN", &component.key, &full, "");
            }
            continue;
        }

        let handle = match scm.open_service_for_ops(
            &svc.name,
            kill_lockers,
            disable_services,
            delete_services,
        ) {
            Ok(h) => h,
            Err(e) => {
                log_action(
                    "Service",
                    "WARN",
                    &component.key,
                    &full,
                    &format!("OpenService failed: {}", format_win_error(e)),
                );
                continue;
            }
        };

        if kill_lockers {
            const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
            match handle.stop() {
                Ok(outcome) => {
                    let detail = match outcome {
                        ffi::StopOutcome::Stopped => "Stopped".to_string(),
                        ffi::StopOutcome::AlreadyStopped => "Already stopped".to_string(),
                        ffi::StopOutcome::StopPendingTimeout => {
                            format!("Still stopping after {SERVICE_STOP_WAIT_MS} ms; continuing")
                        }
                        ffi::StopOutcome::QueryFailed(e) => {
                            format!(
                                "Stop requested; status query failed: {}",
                                format_win_error(e)
                            )
                        }
                    };
                    let level = match outcome {
                        ffi::StopOutcome::Stopped | ffi::StopOutcome::AlreadyStopped => "INFO",
                        _ => "WARN",
                    };
                    log_action("StopService", level, &component.key, &full, &detail);
                }
                Err(ERROR_SERVICE_NOT_ACTIVE) => {
                    log_action("StopService", "INFO", &component.key, &full, "Not running");
                }
                Err(e) => {
                    log_action(
                        "StopService",
                        "WARN",
                        &component.key,
                        &full,
                        &format_win_error(e),
                    );
                }
            }
        }

        if disable_services {
            let res = handle.disable();
            log_action(
                "DisableService",
                if res.is_ok() { "INFO" } else { "WARN" },
                &component.key,
                &full,
                &if res.is_ok() {
                    "Disabled".to_string()
                } else {
                    format_win_error(res.err().unwrap())
                },
            );
        }

        if delete_services {
            let res = handle.delete();
            log_action(
                "DeleteService",
                if res.is_ok() { "INFO" } else { "WARN" },
                &component.key,
                &full,
                &if res.is_ok() {
                    "Deleted".to_string()
                } else {
                    format_win_error(res.err().unwrap())
                },
            );
        }
    }
    log_line("INFO", &stage_summary("Services", &counts));
}

/// Tallies of one service/task stage.
#[derive(Default)]
pub struct StageCounts {
    pub scanned: usize,
    /// Entries with a strong NVIDIA anchor.
    pub nvidia: usize,
    /// Of `nvidia`: owned by an enabled component (will be changed).
    pub targeted: usize,
    /// Of `nvidia`: owned by a deselected component (left alone).
    pub deselected: usize,
    /// Of `nvidia`: the preserved NVIDIA Display Container.
    pub preserved: usize,
}

/// One plain-language line per service/task stage, so the log shows that the
/// stage ran and why each NVIDIA entry was or was not touched.
pub fn stage_summary(stage: &str, c: &StageCounts) -> String {
    if c.nvidia == 0 {
        return format!("{stage}: {} checked, none from NVIDIA.", c.scanned);
    }
    let mut parts: Vec<String> = Vec::new();
    if c.preserved > 0 {
        parts.push(format!("{} kept (NVIDIA Display Container)", c.preserved));
    }
    if c.deselected > 0 {
        parts.push(format!("{} left alone (component off)", c.deselected));
    }
    let unrecognized = c
        .nvidia
        .saturating_sub(c.targeted + c.deselected + c.preserved);
    if unrecognized > 0 {
        parts.push(format!("{unrecognized} not recognized as bloat (left alone)"));
    }
    parts.push(if c.targeted == 0 {
        "nothing to change".to_string()
    } else {
        format!("{} to change", c.targeted)
    });
    format!(
        "{stage}: {} checked, {} from NVIDIA: {}.",
        c.scanned,
        c.nvidia,
        parts.join(", ")
    )
}

const SERVICE_STOP_WAIT_MS: u32 = 10_000;

/// Port of `handleScheduledTasks` (schtasks.exe by absolute System32 path).
/// Every matched task is classified against the component selection first
/// (SAFETY-01); unknown-classification tasks fail closed (never mutated).
pub fn handle_scheduled_tasks(enabled: &crate::app::EnabledMap) {
    let (disable_tasks, delete_tasks, execute) = app::opts(|o| {
        (
            o.disable_scheduled_tasks,
            o.delete_scheduled_tasks,
            o.execute,
        )
    });

    if !disable_tasks && !delete_tasks {
        log_line(
            "INFO",
            "Scheduled tasks: not checked (task handling is turned off).",
        );
        return;
    }

    let task_names: Vec<String> = {
        let com_tasks = ffi::TiSession::connect()
            .map(|s| s.enumerate_all_task_paths())
            .unwrap_or_default();
        if !com_tasks.is_empty() {
            com_tasks
        } else {
            let schtasks = match system_dir_file("schtasks.exe") {
                Ok(p) => p,
                Err(e) => {
                    log_line("ERROR", &format!("Scheduled task handling skipped: {e}"));
                    return;
                }
            };
            let res = run_process_capture(
                &join_command(&[
                    schtasks,
                    "/Query".into(),
                    "/FO".into(),
                    "CSV".into(),
                    "/NH".into(),
                ]),
                30_000,
            );
            if res.exit_code != 0 {
                log_line(
                    "WARN",
                    &format!("schtasks query failed: {}", trim(&res.output)),
                );
                return;
            }
            let mut list = Vec::new();
            for raw_line in res.output.lines() {
                let line = trim(raw_line);
                if line.is_empty() {
                    continue;
                }
                let fields = parse_csv_line(&line);
                if fields.is_empty() {
                    continue;
                }
                let mut task_name = String::new();
                for f in &fields {
                    if f.starts_with('\\') {
                        task_name = f.clone();
                        break;
                    }
                }
                if task_name.is_empty() {
                    task_name = fields[0].clone();
                }
                list.push(task_name);
            }
            list
        }
    };

    let schtasks = system_dir_file("schtasks.exe").ok();

    let mut counts = StageCounts {
        scanned: task_names.len(),
        ..StageCounts::default()
    };
    for task_name in task_names {
        if !ffi_process::has_strong_nvidia_named_target_context(&task_name) {
            continue;
        }
        counts.nvidia += 1;
        let component = match task_action_decision(&task_name, enabled) {
            ActionDecision::Allowed(c) => {
                counts.targeted += 1;
                c
            }
            ActionDecision::ComponentDisabled(c) => {
                counts.deselected += 1;
                log_action(
                    "ScheduledTask",
                    "SKIP",
                    &c.key,
                    &task_name,
                    "component deselected",
                );
                continue;
            }
            ActionDecision::NotMatched => continue,
        };

        if delete_tasks {
            if !execute {
                log_action("DeleteTask", "DRYRUN", &component.key, &task_name, "");
            } else if let Some(ref schtasks) = schtasks {
                let del = run_process_capture(
                    &join_command(&[
                        schtasks.clone(),
                        "/Delete".into(),
                        "/TN".into(),
                        task_name.clone(),
                        "/F".into(),
                    ]),
                    60_000,
                );
                log_action(
                    "DeleteTask",
                    if del.exit_code == 0 { "INFO" } else { "WARN" },
                    &component.key,
                    &task_name,
                    &trim(&del.output),
                );
            }
        } else if disable_tasks {
            if !execute {
                log_action("DisableTask", "DRYRUN", &component.key, &task_name, "");
            } else if let Some(ref schtasks) = schtasks {
                let dis = run_process_capture(
                    &join_command(&[
                        schtasks.clone(),
                        "/Change".into(),
                        "/TN".into(),
                        task_name.clone(),
                        "/Disable".into(),
                    ]),
                    60_000,
                );
                log_action(
                    "DisableTask",
                    if dis.exit_code == 0 { "INFO" } else { "WARN" },
                    &component.key,
                    &task_name,
                    &trim(&dis.output),
                );
            }
        }
    }
    log_line("INFO", &stage_summary("Scheduled tasks", &counts));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_module_is_never_announced_as_reboot_deletion() {
        // Regression: nvprofileupdaterplugin.dll was reported as "deletion may
        // require reboot" / "reboot will finalize" with its component off.
        let (level, msg) =
            loaded_module_message(false, true, false, false, "nvprofileupdaterplugin.dll", 3656)
                .unwrap();
        assert_eq!(level, "INFO");
        assert!(msg.contains("kept (its component is off)"), "{msg}");
        assert!(msg.starts_with("nvprofileupdaterplugin.dll"), "{msg}");
        assert_eq!(loaded_module_message(true, false, false, false, "x.dll", 1), None);
    }

    #[test]
    fn targeted_module_messages_follow_scheduling_outcome() {
        let first = loaded_module_message(false, true, true, false, "_nvtopps.dll", 1).unwrap();
        assert!(first.1.contains("only be deleted after a restart"), "{first:?}");
        let queued = loaded_module_message(true, false, true, true, "_nvtopps.dll", 1).unwrap();
        assert_eq!(queued.0, DETAIL, "the result section already says it");
        let missed = loaded_module_message(true, false, true, false, "_nvtopps.dll", 1).unwrap();
        assert!(missed.1.contains("not queued"), "{missed:?}");
        // Pre-phase repeats stay quiet.
        assert_eq!(loaded_module_message(false, false, true, false, "a.dll", 1), None);
    }

    #[test]
    fn stage_summary_explains_every_nvidia_entry() {
        let none = StageCounts {
            scanned: 276,
            ..StageCounts::default()
        };
        assert_eq!(
            stage_summary("Scheduled tasks", &none),
            "Scheduled tasks: 276 checked, none from NVIDIA."
        );
        // The case from the 2026-10-22 log: one NVIDIA service, the container.
        let container = StageCounts {
            scanned: 299,
            nvidia: 1,
            preserved: 1,
            ..StageCounts::default()
        };
        assert_eq!(
            stage_summary("Services", &container),
            "Services: 299 checked, 1 from NVIDIA: 1 kept (NVIDIA Display Container), nothing to change."
        );
        let mixed = StageCounts {
            scanned: 10,
            nvidia: 4,
            targeted: 1,
            deselected: 1,
            preserved: 1,
        };
        assert_eq!(
            stage_summary("Services", &mixed),
            "Services: 10 checked, 4 from NVIDIA: 1 kept (NVIDIA Display Container), \
             1 left alone (component off), 1 not recognized as bloat (left alone), 1 to change."
        );
    }

    #[test]
    fn container_summary_lists_pids() {
        assert_eq!(container_summary(&[], true), "NVIDIA Display Container: not running.");
        assert_eq!(
            container_summary(&[2800, 3656], true),
            "NVIDIA Display Container: running (PID 2800, 3656), kept."
        );
    }
}
