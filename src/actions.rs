//! Actions on the live system: NVIDIA-container module inspection, locker
//! process termination, bloat service handling, and scheduled-task handling
//! via schtasks.exe. Ports of `inspectNvContainerModules`,
//! `killLockerProcesses`, `handleServices`, `handleScheduledTasks`.

use crate::app;
use crate::ffi;
use crate::logging::{add_action, log_action, log_line};
use crate::matching::{
    is_bloat_process_name, is_nvidia_context_string, is_preserved_container_name,
    module_is_telemetry_or_updater,
};
use crate::procs::run_process_capture;
use crate::sysinfo::system_dir_file;
use crate::util::{contains_no_case, join_command, parse_csv_line, to_lower, trim};
use crate::winfmt::format_win_error;

/// Port of `inspectNvContainerModules` (runs twice per cleanup; dedup sets
/// live in RunState so sightings are not double-reported across phases).
pub fn inspect_nv_container_modules(post_delete_phase: bool) {
    if post_delete_phase {
        log_line(
            "INFO",
            "Re-inspecting NVIDIA Container processes after deletion.",
        );
    } else {
        log_line(
            "INFO",
            "Inspecting NVIDIA Container processes and loaded telemetry/profile-update modules.",
        );
    }

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

    for proc in processes {
        if !is_preserved_container_name(&proc.exe_name) {
            continue;
        }
        let pid_str = proc.pid.to_string();
        let container_key = format!("{}|{}", pid_str, proc.exe_name);
        let first_container_sighting =
            app::run_mut(|s| s.reported_containers.insert(container_key));
        if first_container_sighting {
            log_line(
                "INFO",
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
                log_line(
                    "WARN",
                    &format!(
                        "Module inspection failed for PID={}: {}",
                        proc.pid,
                        format_win_error(e)
                    ),
                );
                continue;
            }
        };
        for path in modules {
            if !module_is_telemetry_or_updater(&path) {
                continue;
            }
            let module_key = format!("{}|{}", pid_str, path);
            let already_reported = !app::run_mut(|s| s.reported_modules.insert(module_key));
            if !already_reported {
                log_line(
                    "WARN",
                    &format!(
                        "Container has matching module loaded; deletion may require reboot: PID={} Module={path}",
                        pid_str
                    ),
                );
                add_action(
                    "LoadedTelemetryModule",
                    "Loaded",
                    "NvContainer",
                    &path,
                    &format!("PID={pid_str}"),
                );
            } else if post_delete_phase {
                log_line(
                    "INFO",
                    &format!(
                        "Module still loaded after deletion (reboot will finalize): PID={} Module={path}",
                        pid_str
                    ),
                );
            }
        }
    }
}

/// Port of `killLockerProcesses`.
pub fn kill_locker_processes() {
    let (kill_lockers, execute, preserve_containers) =
        app::opts(|o| (o.kill_lockers, o.execute, o.preserve_nv_containers));
    if !kill_lockers {
        log_line(
            "INFO",
            "--kill-lockers not specified; process stopping skipped.",
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

    for proc in processes {
        let is_container = is_preserved_container_name(&proc.exe_name);
        if is_container && preserve_containers {
            continue;
        }
        if !is_bloat_process_name(&proc.exe_name) {
            continue;
        }
        if !execute {
            log_action(
                "KillProcess",
                "DRYRUN",
                "Process",
                &proc.exe_name,
                &format!("PID={}", proc.pid),
            );
            continue;
        }
        let Some(handle) = ffi::TerminateHandle::open(proc.pid) else {
            log_action(
                "KillProcess",
                "WARN",
                "Process",
                &proc.exe_name,
                &format!(
                    "OpenProcess failed: {}",
                    format_win_error(ffi::TerminateHandle::last_error())
                ),
            );
            continue;
        };
        let ok = handle.terminate(0);
        let detail = if ok {
            // File handles are released asynchronously; wait briefly so
            // immediate deletions are not defeated by dying lockers.
            if handle.wait_ms(3000) {
                "Terminated".to_string()
            } else {
                "Terminate requested (still exiting)".to_string()
            }
        } else {
            format_win_error(ffi::TerminateHandle::last_error())
        };
        log_action(
            "KillProcess",
            if ok { "INFO" } else { "WARN" },
            "Process",
            &proc.exe_name,
            &detail,
        );
    }
}

/// Port of `serviceMatchesBloat`.
fn service_matches_bloat(
    service_name: &str,
    display_name: &str,
    enabled: &crate::app::EnabledMap,
) -> bool {
    let combined = format!("{service_name} {display_name}");
    if app::opts(|o| o.preserve_nv_containers) && is_preserved_container_name(&combined) {
        return false;
    }
    if !is_nvidia_context_string(&combined) {
        return false;
    }
    const TERMS: &[&str] = &[
        "telemetry",
        "displaydriverras",
        "update",
        "profileupdater",
        "frameview",
        "nvstream",
        "shadowplay",
        "share",
        "ansel",
        "nvidia app",
        "geforce experience",
        "broadcast",
    ];
    if TERMS.iter().any(|t| contains_no_case(&combined, t)) {
        return true;
    }
    enabled.get("VirtualAudio").copied().unwrap_or(false) && contains_no_case(&combined, "nvvad")
}

/// Port of `handleServices`.
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
        log_line("INFO", "Service stopping/disable/delete skipped.");
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

    for svc in services {
        if !service_matches_bloat(&svc.name, &svc.display, enabled) {
            continue;
        }
        let full = format!("{} ({})", svc.name, svc.display);
        if !execute {
            if kill_lockers {
                log_action("StopService", "DRYRUN", "Service", &full, "");
            }
            if disable_services {
                log_action("DisableService", "DRYRUN", "Service", &full, "");
            }
            if delete_services {
                log_action("DeleteService", "DRYRUN", "Service", &full, "");
            }
            continue;
        }

        let handle = match scm.open_service_full(&svc.name) {
            Ok(h) => h,
            Err(e) => {
                log_action(
                    "Service",
                    "WARN",
                    "Service",
                    &full,
                    &format!("OpenService failed: {}", format_win_error(e)),
                );
                continue;
            }
        };

        if kill_lockers {
            let res = handle.stop();
            let ok = res.is_ok();
            let err_ok_not_active = res.as_ref().err().map(|e| *e == 1062).unwrap_or(false);
            let status = if ok || err_ok_not_active {
                "INFO"
            } else {
                "WARN"
            };
            let detail = if ok {
                "Stop requested".to_string()
            } else {
                format_win_error(res.err().unwrap())
            };
            log_action("StopService", status, "Service", &full, &detail);
        }

        if disable_services {
            let res = handle.disable();
            log_action(
                "DisableService",
                if res.is_ok() { "INFO" } else { "WARN" },
                "Service",
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
                "Service",
                &full,
                &if res.is_ok() {
                    "Deleted".to_string()
                } else {
                    format_win_error(res.err().unwrap())
                },
            );
        }
    }
}

/// Port of `taskMatchesBloat`. Token-aware context check: a bare 'nv'
/// substring would also hit unrelated names such as '\Inventory\...' tasks.
fn task_matches_bloat(task_name: &str) -> bool {
    if !is_nvidia_context_string(task_name) {
        return false;
    }
    const TERMS: &[&str] = &[
        "telemetry",
        "update",
        "profile",
        "frameview",
        "shadowplay",
        "share",
        "nvstream",
        "geforce",
        "nvidia app",
        "displaydriverras",
    ];
    let l = to_lower(task_name);
    TERMS.iter().any(|t| l.contains(t))
}

/// Port of `handleScheduledTasks` (schtasks.exe by absolute System32 path).
pub fn handle_scheduled_tasks() {
    let (disable_tasks, delete_tasks, execute) = app::opts(|o| {
        (
            o.disable_scheduled_tasks,
            o.delete_scheduled_tasks,
            o.execute,
        )
    });

    if !disable_tasks && !delete_tasks {
        log_line("INFO", "Scheduled task handling skipped.");
        return;
    }

    let schtasks = system_dir_file("schtasks.exe");
    let res = run_process_capture(
        &join_command(&[
            schtasks.clone(),
            "/Query".into(),
            "/FO".into(),
            "CSV".into(),
            "/NH".into(),
        ]),
        120_000,
    );
    if res.exit_code != 0 {
        log_line(
            "WARN",
            &format!("schtasks query failed: {}", trim(&res.output)),
        );
        return;
    }

    for raw_line in res.output.lines() {
        let line = trim(raw_line);
        if line.is_empty() {
            continue;
        }
        let fields = parse_csv_line(&line);
        if fields.is_empty() {
            continue;
        }
        // The task-name column position varies between schtasks versions;
        // select the field that looks like an absolute task path ('\Foo').
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
        if !task_matches_bloat(&task_name) {
            continue;
        }

        if delete_tasks {
            if !execute {
                log_action("DeleteTask", "DRYRUN", "ScheduledTask", &task_name, "");
            } else {
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
                    "ScheduledTask",
                    &task_name,
                    &trim(&del.output),
                );
            }
        } else if disable_tasks {
            if !execute {
                log_action("DisableTask", "DRYRUN", "ScheduledTask", &task_name, "");
            } else {
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
                    "ScheduledTask",
                    &task_name,
                    &trim(&dis.output),
                );
            }
        }
    }
}
