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
mod ffi_services;
mod ffi_tasksched;
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
    EXIT_ABORTED, EXIT_ALREADY_RUNNING, EXIT_FATAL_EXCEPTION, EXIT_FATAL_UNKNOWN, EXIT_OK,
    EXIT_TI_CHILD_FAILED, EXIT_TI_RELAUNCH_FAILED,
};
use logging::log_line;
use options::{apply_component_args, initialize_component_selection, parse_args, print_usage};
use sysinfo::is_trusted_installer;
use types::Options;

extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            app::ABORT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
            1 // TRUE: handled; stop-after-current-item semantics
        }
        _ => 0, // FALSE: not handled
    }
}

/// Status JSON writer (port of `writeStatusJson`). Field names and layout are
/// a cross-version contract — do not change.
fn write_status_json(exit_code: i32, status: &str, detail: &str) {
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
    let _ = std::fs::write(path, json);
}

/// Outcome of the inner run: `Early(code)` mirrors C++ `return code;` inside
/// the try block (which SKIPS the pause block), `Completed(code)` falls
/// through to the pause block, mirroring end-of-try / catch paths.
enum Flow {
    Early(i32),
    Completed(i32),
}

/// Port of the wmain try-block. Errors returned as Err(String) map onto the
/// C++ catch(std::exception) path.
fn wmain_try(args: &[String]) -> Result<Flow, String> {
    let opts: Options = parse_args(args);

    if opts.show_help {
        print_usage();
        return Ok(Flow::Early(EXIT_OK));
    }
    if opts.show_version {
        console::out(&format!(
            "GreenPostInstallDebloatNative version {}\n",
            app::GPD_VERSION
        ));
        return Ok(Flow::Early(EXIT_OK));
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
        return Ok(Flow::Early(EXIT_OK));
    }

    apply_component_args(args);

    // Wizard default keeps the NVIDIA driver-update/profile-updater stack:
    // deselect UpdateAndProfileUpdater unless explicitly re-enabled.
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
        return Ok(Flow::Early(EXIT_OK));
    }

    let log_path_display = app::run(|s| s.log_path.to_string_lossy().into_owned());
    log_line(
        "INFO",
        &format!("Run log (all stages of this run append to this single file): {log_path_display}"),
    );
    for unknown in app::opts(|o| o.unknown_args.clone()) {
        log_line("WARN", &format!("Ignoring unknown option: {unknown}"));
    }

    // Bare double-click launch in EXECUTE mode without elevation: the wizard
    // has been confirmed interactively at this point, so hand off to an
    // elevated instance via UAC. Must happen before the single-instance mutex
    // is taken, otherwise the elevated child would see it as held.
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
                return Ok(Flow::Early(child_exit));
            }
            None => {
                log_line(
                    "WARN",
                    "Elevation was declined or unavailable; continuing without administrator rights.",
                );
                console::out(
                    "\nElevation was declined or unavailable; continuing without administrator rights.\n",
                );
            }
        }
    }

    // Refuse overlapping destructive runs; they would race over files,
    // services and pending-reboot registrations.
    let _mutex_guard = if app::opts(|o| o.execute && !o.ti_child) {
        let (guard, existed) =
            ffi::create_global_mutex("Global\\GreenPostInstallDebloatNative_Execute_Mutex");
        if !guard.held() {
            log_line(
                "WARN",
                &format!(
                    "Single-instance mutex unavailable: {}",
                    winfmt::format_win_error(ffi::current_last_error())
                ),
            );
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
            return Ok(Flow::Early(EXIT_ALREADY_RUNNING));
        }
        Some(guard)
    } else {
        None
    };

    let should_attempt_ti =
        app::opts(|o| o.execute && !o.ti_child && !o.allow_admin_fallback && o.attempt_ti_relaunch)
            && !is_trusted_installer();
    if should_attempt_ti {
        let ti_result = tasksched::attempt_trusted_installer_relaunch();
        if ti_result.child_status_seen && ti_result.child_succeeded {
            log_line(
                "INFO",
                "Parent process finished after TI child completion. Everything (including the worker's report) is in the shared run log.",
            );
            return Ok(Flow::Early(EXIT_OK));
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
            return Ok(Flow::Early(EXIT_TI_CHILD_FAILED));
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
            return Ok(Flow::Early(EXIT_TI_RELAUNCH_FAILED));
        }
    }

    match report::run_cleanup() {
        Ok(()) => {
            let aborted = app::run(|s| s.aborted);
            if aborted {
                write_status_json(EXIT_ABORTED, "aborted", "run aborted by user request");
                Ok(Flow::Completed(EXIT_ABORTED))
            } else {
                write_status_json(EXIT_OK, "ok", "completed");
                Ok(Flow::Completed(EXIT_OK))
            }
        }
        Err(msg) => Err(msg),
    }
}

/// Port of `relaunchElevatedForWizard`. Returns child exit code when the UAC
/// relaunch succeeded; None when elevation was declined/unavailable.
fn relaunch_elevated_for_wizard() -> Option<i32> {
    let mut params = tasksched::effective_child_switches().join(" ");
    params.push_str(" --no-menu --pause");
    // All stages append to this same log file.
    let log_file = app::run(|s| s.log_path.to_string_lossy().into_owned());
    params.push_str(&format!(" --log-file \"{log_file}\""));

    let exe_path = sysinfo::get_exe_path().to_string_lossy().into_owned();
    let child = ffi::shellexecute_runas(&exe_path, &params)?;
    Some(child.wait_exit_code() as i32)
}

/// Pause helper mirroring the tail of wmain.
fn maybe_pause_on_exit() {
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

    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wmain_try(&args)));

    let exit_code = match result {
        Ok(Ok(flow)) => flow,
        Ok(Err(msg)) => {
            // catch(std::exception) — message available.
            logging::log_line("FATAL", &msg);
            let _ = std::panic::catch_unwind(report::write_report);
            write_status_json(EXIT_FATAL_EXCEPTION, "fatal", &msg);
            Flow::Completed(EXIT_FATAL_EXCEPTION)
        }
        Err(_payload) => {
            // catch(...) — unknown fatal exception.
            logging::log_line("FATAL", "Unknown fatal exception.");
            let _ = std::panic::catch_unwind(report::write_report);
            write_status_json(EXIT_FATAL_UNKNOWN, "fatal", "unknown fatal exception");
            Flow::Completed(EXIT_FATAL_UNKNOWN)
        }
    };

    match exit_code {
        Flow::Early(code) => std::process::exit(code),
        Flow::Completed(code) => {
            maybe_pause_on_exit();
            std::process::exit(code);
        }
    }
}
