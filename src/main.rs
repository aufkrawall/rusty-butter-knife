//! Program entry point and orchestration. Port of `wmain` including the
//! exact early-return/pause control flow, single-instance mutex, UAC relaunch,
//! and TrustedInstaller relaunch decision tree.

#![deny(unsafe_code)]

mod actions;
mod app;
mod components;
mod console;
mod deletion;
mod discovery;
mod ffi;
mod ffi_capture;
mod ffi_process;
mod ffi_services;
mod ffi_tasksched;
mod fsutil;
mod logging;
mod matching;
mod menu;
mod options;
mod procs;
mod report;
mod sysinfo;
mod tasksched;
mod types;
mod util;
mod winfmt;

use app::{
    EXIT_ABORTED, EXIT_ALREADY_RUNNING, EXIT_BAD_ARGS, EXIT_FATAL_EXCEPTION, EXIT_FATAL_UNKNOWN,
    EXIT_OK, EXIT_TI_CHILD_FAILED, EXIT_TI_RELAUNCH_FAILED,
};
use logging::log_line;
use options::{
    apply_component_args_and_collect_problems, initialize_component_selection, parse_args,
    print_usage,
};
use sysinfo::is_trusted_installer;
use types::Options;

extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            app::ABORT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
            1
        }
        _ => 0,
    }
}

/// Status JSON writer. Field names and layout are a cross-version contract.
fn write_status_json(exit_code: i32, status: &str, detail: &str) {
    if !app::opts_initialized() {
        return;
    }
    let status_file = app::opts(|o| o.status_file.clone());
    if status_file.is_empty() {
        return;
    }
    let (run_id, log_path) =
        app::run(|s| (s.run_id.clone(), s.log_path.to_string_lossy().into_owned()));
    let path = std::path::Path::new(&status_file);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let json = format!(
        "{{\n  \"version\": \"{}\",\n  \"runId\": \"{}\",\n  \"exitCode\": {},\n  \"status\": \"{}\",\n  \"detail\": \"{}\",\n  \"logPath\": \"{}\"\n}}\n",
        app::GPD_VERSION,
        util::json_escape(&run_id),
        exit_code,
        util::json_escape(status),
        util::json_escape(detail),
        util::json_escape(&log_path),
    );
    if let Err(e) = std::fs::write(path, json) {
        console::err_out(&format!(
            "ERROR: failed to write status file {}: {}\n",
            path.display(),
            e
        ));
    }
}

fn wmain_try(args: &[String]) -> Result<i32, String> {
    let opts: Options = parse_args(args);

    if opts.show_help {
        print_usage();
        return Ok(EXIT_OK);
    }
    if opts.show_version {
        console::out(&format!(
            "Rusty Butter Knife version {}\n",
            app::RBK_VERSION
        ));
        return Ok(EXIT_OK);
    }

    app::init_app(opts);
    initialize_component_selection();

    if app::opts(|o| o.list_components) {
        for c in components::build_components() {
            let on = app::enabled(|m| m.get(&c.key).copied().unwrap_or(false));
            console::out(&format!(
                "[{}] {}{}\n",
                if on { "x" } else { " " },
                c.key,
                if c.optional { " (optional)" } else { "" }
            ));
        }
        return Ok(EXIT_OK);
    }

    apply_component_args_and_collect_problems(args);

    if app::opts(|o| o.wizard_defaults) {
        app::enabled_mut(|m| {
            if let Some(v) = m.get_mut("UpdateAndProfileUpdater") {
                *v = false;
            }
        });
    }

    sysinfo::initialize_run_state();

    if app::opts(|o| o.menu && !o.ti_child) {
        menu::interactive_menu();
    }
    if app::user_quit_requested() {
        return Ok(EXIT_OK);
    }

    let log_path_display = app::run(|s| s.log_path.to_string_lossy().into_owned());
    log_line(
        "INFO",
        &format!("Run log (all stages of this run append to this single file): {log_path_display}"),
    );

    let bad_args = app::opts(|o| o.unknown_args.clone());
    if !bad_args.is_empty() {
        let execute_mode = app::opts(|o| o.execute);
        for bad in &bad_args {
            if execute_mode {
                log_line("ERROR", &format!("Invalid or unknown argument: {bad}"));
            } else {
                log_line(
                    "WARN",
                    &format!("Ignoring invalid or unknown argument: {bad}"),
                );
            }
        }
        if execute_mode {
            console::out("Run with --help to see valid arguments.\n");
            write_status_json(
                EXIT_BAD_ARGS,
                "failed",
                "invalid or unknown command-line arguments",
            );
            return Ok(EXIT_BAD_ARGS);
        }
    }

    if app::opts(|o| o.wizard_defaults && o.execute && !o.ti_child)
        && !sysinfo::is_admin()
        && !is_trusted_installer()
    {
        log_line(
            "INFO",
            "Administrator rights required; relaunching elevated via UAC with the confirmed selection.",
        );
        match relaunch_elevated_for_wizard() {
            Some(child_exit) => {
                log_line(
                    "INFO",
                    &format!("Elevated instance finished with exit code {child_exit}."),
                );
                return Ok(child_exit);
            }
            None => {
                log_line(
                    "WARN",
                    "Elevation handoff was declined or unavailable; continuing without administrator rights.",
                );
                console::out(
                    "\nElevation handoff was declined or unavailable; continuing without administrator rights.\n",
                );
            }
        }
    }

    let _mutex_guard = if app::opts(|o| o.execute && !o.ti_child) {
        let (guard, existed) =
            ffi::create_global_mutex("Global\\RustyButterKnife_Execute_Mutex");
        if !guard.held() {
            log_line(
                "FATAL",
                &format!(
                    "Single-instance mutex unavailable: {}; refusing concurrent destructive run.",
                    winfmt::format_win_error(ffi::current_last_error())
                ),
            );
            write_status_json(
                EXIT_ALREADY_RUNNING,
                "failed",
                "single-instance execute-mode mutex unavailable",
            );
            return Ok(EXIT_ALREADY_RUNNING);
        } else if existed {
            log_line(
                "FATAL",
                "Another execute-mode instance is already running; refusing concurrent destructive run.",
            );
            write_status_json(
                EXIT_ALREADY_RUNNING,
                "failed",
                "another execute-mode instance is running",
            );
            return Ok(EXIT_ALREADY_RUNNING);
        }
        Some(guard)
    } else {
        None
    };

    let ti_preflight = app::opts(|o| o.execute && !o.ti_child && !o.allow_admin_fallback)
        && !is_trusted_installer();
    if ti_preflight {
        if !app::opts(|o| o.attempt_ti_relaunch) {
            log_line(
                "FATAL",
                "Destructive execution requires TrustedInstaller, but relaunch was disabled (--no-ti-relaunch) and --allow-admin-fallback was not specified.",
            );
            write_status_json(
                EXIT_TI_RELAUNCH_FAILED,
                "failed",
                "TI relaunch disabled without admin fallback",
            );
            return Ok(EXIT_TI_RELAUNCH_FAILED);
        }
        {
            let ti_result = tasksched::attempt_trusted_installer_relaunch();
            if ti_result.child_status_seen && ti_result.child_succeeded {
                log_line(
                    "INFO",
                    "Parent process finished after TI child completion. Everything (including the worker's report) is in the shared run log.",
                );
                write_status_json(EXIT_OK, "ok", "TI child completed");
                return Ok(EXIT_OK);
            }
            if ti_result.child_status_seen {
                log_line(
                    "FATAL",
                    &format!("Elevated TI child reported failure: {}", ti_result.detail),
                );
                write_status_json(
                    EXIT_TI_CHILD_FAILED,
                    "failed",
                    &format!("TI child failed: {}", ti_result.detail),
                );
                return Ok(EXIT_TI_CHILD_FAILED);
            }
            if ti_result.aborted {
                write_status_json(EXIT_ABORTED, "aborted", &ti_result.detail);
                return Ok(EXIT_ABORTED);
            }
            if !app::opts(|o| o.allow_admin_fallback) {
                let detail_suffix = if ti_result.detail.is_empty() {
                    ".".to_string()
                } else {
                    format!(" ({})", ti_result.detail)
                };
                log_line(
                    "FATAL",
                    &format!(
                        "TrustedInstaller relaunch failed{detail_suffix} and --allow-admin-fallback was not specified."
                    ),
                );
                write_status_json(
                    EXIT_TI_RELAUNCH_FAILED,
                    "failed",
                    &format!("TrustedInstaller relaunch failed: {}", ti_result.detail),
                );
                return Ok(EXIT_TI_RELAUNCH_FAILED);
            }
        }
    }

    match report::run_cleanup() {
        Ok(()) => {
            let aborted = app::run(|s| s.aborted);
            if aborted {
                write_status_json(EXIT_ABORTED, "aborted", "run aborted by user request");
                Ok(EXIT_ABORTED)
            } else {
                write_status_json(EXIT_OK, "ok", "completed");
                Ok(EXIT_OK)
            }
        }
        Err(msg) => Err(msg),
    }
}

/// Create the per-run manual-reset event used to relay Ctrl+C from the
/// unelevated launcher to the elevated UAC child. If creation fails, the UAC
/// handoff is refused rather than reintroducing detached-abort semantics.
fn prepare_abort_relay_event() -> Option<(String, ffi_process::NamedAbortEvent)> {
    let name = format!(
        "Local\\RustyButterKnife_Abort_{}",
        util::now_unique_suffix()
    );
    let event = ffi_process::NamedAbortEvent::create(&name)?;
    app::opts_mut(|o| o.abort_event = name.clone());
    Some((name, event))
}

/// UAC handoff. Ctrl+C in the launcher signals a named kernel event observed
/// by the elevated process. The launcher keeps waiting until that process
/// actually exits instead of reporting "aborted" while destructive work runs.
fn relaunch_elevated_for_wizard() -> Option<i32> {
    let (abort_event_name, abort_event) = match prepare_abort_relay_event() {
        Some(v) => v,
        None => {
            log_line(
                "ERROR",
                "Could not create cross-process abort event; refusing UAC destructive handoff.",
            );
            return None;
        }
    };

    let mut params = tasksched::effective_child_switches();
    params.push("--no-menu".into());
    params.push("--pause".into());

    let log_file = app::run(|s| s.log_path.to_string_lossy().into_owned());
    params.push("--log-file".into());
    params.push(log_file);

    let status_file = app::opts(|o| o.status_file.clone());
    if !status_file.is_empty() {
        params.push("--status-file".into());
        params.push(status_file);
    }

    params.push("--abort-event".into());
    params.push(abort_event_name);
    let joined = util::join_command(&params);

    let exe_path = sysinfo::get_exe_path().to_string_lossy().into_owned();
    let child = ffi::shellexecute_runas(&exe_path, &joined)?;

    let res = child.wait_exit_code_aborting(|| {
        if app::ABORT_REQUESTED.local_requested(std::sync::atomic::Ordering::SeqCst) {
            let _ = abort_event.signal();
        }
        // Never abandon the elevated child. The event requests cooperative
        // cancellation; this launcher remains attached until child exit.
        false
    });
    Some(res as i32)
}

fn maybe_pause_on_exit() {
    if !app::opts_initialized() {
        return;
    }
    let (no_pause, ti_child, pause_on_exit) =
        app::opts(|o| (o.no_pause, o.ti_child, o.pause_on_exit));
    if !no_pause
        && !ti_child
        && !app::user_quit_requested()
        && (app::opts(|o| o.menu) || pause_on_exit)
    {
        console::out("\nPress Enter to exit...");
        console::flush();
        let _ = console::read_line();
    }
}

fn main() {
    ffi::set_console_ctrl_handler(console_ctrl_handler);

    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wmain_try(&args)));

    let exit_code = match result {
        Ok(Ok(code)) => code,
        Ok(Err(msg)) => {
            logging::log_line("FATAL", &msg);
            let _ = std::panic::catch_unwind(report::write_report);
            write_status_json(EXIT_FATAL_EXCEPTION, "fatal", &msg);
            EXIT_FATAL_EXCEPTION
        }
        Err(_payload) => {
            logging::log_line("FATAL", "Unknown fatal exception.");
            let _ = std::panic::catch_unwind(report::write_report);
            write_status_json(EXIT_FATAL_UNKNOWN, "fatal", "unknown fatal exception");
            EXIT_FATAL_UNKNOWN
        }
    };

    maybe_pause_on_exit();
    std::process::exit(exit_code);
}
