//! CLI parsing: `parseArgs`, `printUsage`, `parseBoolAssignment`,
//! `applyComponentArgs`.

use crate::app;
use crate::components::build_components;
use crate::console;
use crate::types::Options;
use crate::util::{atoi_prefix, to_lower};

/// Port of `printUsage` (text kept byte-identical; build tag updated to Rust).
pub fn print_usage() {
    let text = format!(
        "\nNVIDIA post-install debloater native Rust build (version {})\n\n\
Usage:\n\
  GreenPostInstallDebloatNative.exe                      Bare launch (double-click): wizard with recommended\n\
                                                         defaults pre-selected (execute, kill-lockers, disable-services,\n\
                                                         delete-scheduled-tasks, reboot-delete); keeps the\n\
                                                         NVIDIA profile updater installed. Requires interactive\n\
                                                         confirmation; elevates via UAC when needed.\n\
  GreenPostInstallDebloatNative.exe --menu               Interactive wizard with inert dry-run defaults.\n\
  GreenPostInstallDebloatNative.exe --dry-run            Safe scan without changes.\n\
  GreenPostInstallDebloatNative.exe --execute --kill-lockers --disable-services --delete-scheduled-tasks --schedule-reboot-delete\n\n\
Core switches:\n\
  --execute                       Actually delete/disable. Default is dry-run.\n\
                                  Exception: a bare launch (no arguments) opens the wizard\n\
                                  in EXECUTE mode with the recommended switches pre-selected.\n\
  --dry-run                       Explicit dry-run.\n\
  --menu                          Interactive component selection.\n\
  --no-menu                       Suppress the interactive menu.\n\
  --kill-lockers                  Stop bloat services/processes before deleting.\n\
  --preserve-nvcontainers[=on/off] Allow killing NVDisplay.Container/nvcontainer if matched. Default preserves them.\n\
  --disable-services              Disable matching NVIDIA bloat services.\n\
  --delete-services               Delete matching NVIDIA bloat services.\n\
  --disable-scheduled-tasks       Disable matching NVIDIA scheduled tasks. Default true.\n\
  --no-disable-scheduled-tasks    Do not disable matched scheduled tasks.\n\
  --delete-scheduled-tasks        Delete matching NVIDIA scheduled tasks.\n\
  --schedule-reboot-delete        Use MoveFileEx for locked files/folders.\n\
  --take-ownership                Run takeown/icacls before deletion. Usually unnecessary under TrustedInstaller.\n\n\
TrustedInstaller behavior:\n\
  --no-ti-relaunch                Do not attempt automatic TrustedInstaller scheduled-task relaunch.\n\
  --allow-admin-fallback          Permit destructive execution as Administrator if TI relaunch fails/skipped.\n\
  --ti-wait-seconds N             Parent wait timeout for TI child. Default 600.\n\
                                  Component selections made in the interactive menu are forwarded to the elevated child.\n\n\
Optional component inclusions:\n\
  --include-ngx --include-hdaudio --include-physx --include-notebook-optimus\n\
  --include-virtual-audio        Include NvVAD/NVIDIA Virtual Audio Device cleanup. Off by default.\n\
  --include-nvwmi                Include NVIDIA WMI management interface cleanup. Off by default.\n\
  --include-capture-sdk          Include NvFBC/NvIFR capture SDK runtime cleanup. Off by default.\n\n\
Component selection:\n\
  --component=Key:on/off         Toggle a specific component (repeatable).\n\
  --list-components              Print all component keys and their state, then exit.\n\n\
Miscellaneous:\n\
  --status-file PATH             Write child run status JSON to PATH (legacy diagnostics handoff).\n\
  --log-dir PATH                 Base directory for the log file. Default: beside the executable.\n\
  --log-file PATH                Use exactly this log file; all processes of a run (launcher, elevated\n\
                                 instance, SYSTEM worker) append to it, so one run leaves one log.\n\
  --pause                        Always pause on exit (also used by the elevated\n\
                                 instance spawned by a bare double-click launch).\n\
  --no-pause                     Do not pause on exit after an interactive menu run.\n\
  --no-color                     Monochrome console output.\n\
  --version                      Print version and exit.\n\
  --help, -h, /?                 Show this help.\n\n\
Exit codes:\n\
  0 success | 1 fatal exception | 2 unknown fatal | 3 aborted by user\n\
  10 TrustedInstaller relaunch failed/not permitted | 11 elevated child reported failure\n\
  12 another execute-mode instance is already running\n\n",
        crate::app::GPD_VERSION
    );
    console::out(&text);
}

/// Port of `parseBoolAssignment`.
pub fn parse_bool_assignment(arg: &str, name: &str) -> Option<bool> {
    let low = to_lower(arg);
    let prefix = format!("{}=", to_lower(name));
    let v = low.strip_prefix(&prefix)?;
    Some(!(v == "false" || v == "0" || v == "no" || v == "off"))
}

/// Port of `parseArgs`.
pub fn parse_args(args: &[String]) -> Options {
    let mut opt = Options::default();
    if args.is_empty() {
        // Bare launch (e.g. double-click in Explorer): open the wizard with the
        // recommended cleanup pre-selected instead of an inert dry-run.
        // UpdateAndProfileUpdater is deselected via wizardDefaults in main.
        opt.menu = true;
        opt.execute = true;
        opt.kill_lockers = true;
        opt.disable_services = true;
        opt.delete_scheduled_tasks = true;
        opt.schedule_locked_for_reboot = true;
        opt.wizard_defaults = true;
    }
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].clone();
        let low = to_lower(&a);
        if low == "--help" || low == "-h" || low == "/?" {
            opt.show_help = true;
        } else if low == "--version" {
            opt.show_version = true;
        } else if low == "--list-components" {
            opt.list_components = true;
        } else if low == "--menu" {
            opt.menu = true;
        } else if low == "--no-menu" {
            opt.menu = false;
        } else if low == "--execute" || low == "-execute" {
            opt.execute = true;
        } else if low == "--dry-run" || low == "-dryrun" {
            opt.execute = false;
        } else if low == "--allow-admin-fallback" {
            opt.allow_admin_fallback = true;
        } else if low == "--no-ti-relaunch" {
            opt.attempt_ti_relaunch = false;
        } else if low == "--ti-child" {
            opt.ti_child = true;
        } else if low == "--kill-lockers" {
            opt.kill_lockers = true;
        } else if low == "--disable-services" {
            opt.disable_services = true;
        } else if low == "--delete-services" {
            opt.delete_services = true;
        } else if low == "--disable-scheduled-tasks" {
            opt.disable_scheduled_tasks = true;
        } else if low == "--no-disable-scheduled-tasks" {
            opt.disable_scheduled_tasks = false;
        } else if low == "--delete-scheduled-tasks" {
            opt.delete_scheduled_tasks = true;
        } else if low == "--schedule-reboot-delete" {
            opt.schedule_locked_for_reboot = true;
        } else if low == "--take-ownership" {
            opt.take_ownership = true;
        } else if low == "--include-ngx" {
            opt.include_ngx = true;
        } else if low == "--include-hdaudio" {
            opt.include_hd_audio = true;
        } else if low == "--include-physx" {
            opt.include_physx = true;
        } else if low == "--include-notebook-optimus" {
            opt.include_notebook_optimus = true;
        } else if low == "--include-virtual-audio" {
            opt.include_virtual_audio = true;
        } else if low == "--include-nvwmi" {
            opt.include_nvwmi = true;
        } else if low == "--include-capture-sdk" {
            opt.include_capture_sdk = true;
        } else if low == "--no-pause" {
            opt.no_pause = true;
        } else if low == "--pause" {
            opt.pause_on_exit = true;
        } else if low == "--no-color" {
            opt.no_color = true;
        } else if let Some(v) = parse_bool_assignment(&a, "--preserve-nvcontainers") {
            opt.preserve_nv_containers = v;
        } else if low == "--preserve-nvcontainers" {
            opt.preserve_nv_containers = true;
        } else if (low == "--status-file"
            || low == "--log-dir"
            || low == "--ti-wait-seconds"
            || low == "--log-file")
            && i + 1 < args.len()
        {
            i += 1;
            let v = args[i].clone();
            match low.as_str() {
                "--status-file" => opt.status_file = v,
                "--log-dir" => opt.log_dir_override = v,
                "--log-file" => opt.log_file_override = v,
                "--ti-wait-seconds" => opt.ti_wait_seconds = 15.max(atoi_prefix(&v)),
                _ => {}
            }
        } else if let Some(v) = a.strip_prefix("--ti-wait-seconds=") {
            opt.ti_wait_seconds = 15.max(atoi_prefix(v));
        } else if let Some(v) = a.strip_prefix("--status-file=") {
            opt.status_file = v.to_string();
        } else if let Some(v) = a.strip_prefix("--log-file=") {
            opt.log_file_override = v.to_string();
        } else if let Some(v) = a.strip_prefix("--log-dir=") {
            opt.log_dir_override = v.to_string();
        } else if low.starts_with("--component=") {
            // Parsed after component map exists. Format: --component=Key:on/off
        } else {
            opt.unknown_args.push(a.clone());
            console::err_out(&format!("Unknown option: {a}\n"));
        }
        i += 1;
    }
    opt
}

/// Port of `applyComponentArgs` (case-insensitive key matching).
pub fn apply_component_args(args: &[String]) {
    for a in args {
        let Some(spec) = a.strip_prefix("--component=") else {
            continue;
        };
        let low_a = to_lower(a);
        if !low_a.starts_with("--component=") {
            continue;
        }
        let sep = spec.find([':', '=']);
        let Some(sep) = sep else { continue };
        let key = &spec[..sep];
        let val = to_lower(&spec[sep + 1..]);
        let on = !(val == "false" || val == "0" || val == "off" || val == "no");
        let key_low = to_lower(key);
        let mut matched = false;
        app::enabled_mut(|m| {
            for (k, v) in m.iter_mut() {
                if to_lower(k) == key_low {
                    *v = on;
                    matched = true;
                }
            }
        });
        if !matched {
            let valid = app::enabled(|m| {
                let mut valid = String::new();
                for k in m.keys() {
                    if !valid.is_empty() {
                        valid.push_str(", ");
                    }
                    valid.push_str(k);
                }
                valid
            });
            console::err_out(&format!(
                "Warning: unknown component key in '{a}'. Valid keys: {valid}\n"
            ));
        }
    }
}

/// Port of `initializeComponentSelection` (defaults + --include-* overrides).
pub fn initialize_component_selection() {
    let include = app::opts(|o| {
        (
            o.include_ngx,
            o.include_hd_audio,
            o.include_physx,
            o.include_notebook_optimus,
            o.include_virtual_audio,
            o.include_nvwmi,
            o.include_capture_sdk,
        )
    });
    app::enabled_mut(|m| {
        for c in build_components() {
            let mut enabled = c.default_enabled;
            if c.key == "NGX" {
                enabled = include.0;
            }
            if c.key == "HDAudio" {
                enabled = include.1;
            }
            if c.key == "PhysX" {
                enabled = include.2;
            }
            if c.key == "NotebookOptimus" {
                enabled = include.3;
            }
            if c.key == "VirtualAudio" {
                enabled = include.4;
            }
            if c.key == "NvWMI" {
                enabled = include.5;
            }
            if c.key == "CaptureSDK" {
                enabled = include.6;
            }
            m.insert(c.key.clone(), enabled);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_assignment_variants() {
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=off", "--preserve-nvcontainers"),
            Some(false)
        );
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=ON", "--preserve-nvcontainers"),
            Some(true)
        );
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=0", "--preserve-nvcontainers"),
            Some(false)
        );
        // Bare form has no '=' assignment.
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers", "--preserve-nvcontainers"),
            None
        );
        // Different flag never matches.
        assert_eq!(
            parse_bool_assignment("--component=x:on", "--preserve-nvcontainers"),
            None
        );
    }
}
