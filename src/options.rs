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
  --ti-wait-seconds N             Parent wait timeout for TI child. Default 600;
                                  values are clamped to 15..=7200 seconds.\n\
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
  12 another execute-mode instance is already running | 13 invalid or unknown argument\n\n",
        crate::app::GPD_VERSION
    );
    console::out(&text);
}

/// Port of `parseBoolAssignment`. STRICT variant: recognized boolean words
/// only; anything else is malformed so destructive mode can reject it
/// instead of silently treating garbage as "on".
#[derive(Debug, PartialEq, Eq)]
pub enum BoolAssign {
    /// Argument does not carry this flag's assignment at all.
    NotMine,
    /// Well-formed `<flag>=on|off|true|false|1|0|yes|no` (any case).
    Assigned(bool),
    /// `<flag>=<garbage>` present but unparsable.
    Malformed(String),
}

pub fn parse_bool_assignment(arg: &str, name: &str) -> BoolAssign {
    let low = to_lower(arg);
    let prefix = format!("{}=", to_lower(name));
    let v = match low.strip_prefix(&prefix) {
        Some(v) => v,
        None => return BoolAssign::NotMine,
    };
    match v {
        "true" | "1" | "yes" | "on" => BoolAssign::Assigned(true),
        "false" | "0" | "no" | "off" => BoolAssign::Assigned(false),
        other => BoolAssign::Malformed(other.to_string()),
    }
}

/// Numeric-or-error variant of `atoi_prefix`: `None` when the input carries
/// no digits at all (e.g. `--ti-wait-seconds=abc`).
fn seconds_value_or_none(s: &str) -> Option<i64> {
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    if !has_digit {
        return None;
    }
    Some(atoi_prefix(s))
}

const TI_WAIT_MIN_SECS: i64 = 15;
/// The scheduled task itself self-limits at PT2H; waiting longer than 2 h
/// cannot observe a live child anymore, so clamp there.
const TI_WAIT_MAX_SECS: i64 = 7200;

fn apply_ti_wait_seconds(raw: &str, opt: &mut Options) -> Option<String> {
    match seconds_value_or_none(raw) {
        Some(v) => {
            opt.ti_wait_seconds = v.clamp(TI_WAIT_MIN_SECS, TI_WAIT_MAX_SECS);
            None
        }
        None => Some(format!(
            "--ti-wait-seconds={raw} is not a number (allowed range {TI_WAIT_MIN_SECS}..={TI_WAIT_MAX_SECS})"
        )),
    }
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
        } else if let BoolAssign::Assigned(v) = parse_bool_assignment(&a, "--preserve-nvcontainers")
        {
            opt.preserve_nv_containers = v;
        } else if let BoolAssign::Malformed(_v) =
            parse_bool_assignment(&a, "--preserve-nvcontainers")
        {
            opt.unknown_args.push(a.clone());
            console::err_out(&format!(
                "Invalid value in '{a}' (use on/off/true/false/1/0/yes/no)\n"
            ));
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
            // A value that itself looks like a flag means the flag's value is
            // missing. Record it as a problem so execute mode fails closed
            // instead of silently swallowing the next switch (e.g.
            // "--log-file --dry-run" would otherwise drop the dry-run
            // request from a destructive run).
            if v.starts_with('-') && v.len() > 1 {
                opt.unknown_args.push(format!(
                    "{a} is missing its value (found flag-like '{v}' instead)"
                ));
            } else {
                match low.as_str() {
                    "--status-file" => opt.status_file = v,
                    "--log-dir" => opt.log_dir_override = v,
                    "--log-file" => opt.log_file_override = v,
                    "--ti-wait-seconds" => {
                        if let Some(problem) = apply_ti_wait_seconds(&v, &mut opt) {
                            opt.unknown_args.push(problem);
                        }
                    }
                    _ => {}
                }
            }
        } else if let Some(v) = a.strip_prefix("--ti-wait-seconds=") {
            if let Some(problem) = apply_ti_wait_seconds(v, &mut opt) {
                opt.unknown_args.push(problem);
            }
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

/// Port of `applyComponentArgs` (case-insensitive key AND flag spelling).
/// Returns problems instead of only printing warnings, so execute mode can
/// reject malformed component selections before any mutation.
pub fn apply_component_args(args: &[String]) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    for a in args {
        // Mixed-case spellings such as --COMPONENT=... previously fell
        // through silently; route via the lowercased prefix.
        if !to_lower(a).starts_with("--component=") {
            continue;
        }
        let spec = &a["--component=".len()..];
        let sep = spec.find([':', '=']);
        let Some(sep) = sep else {
            problems.push(format!(
                "{a} is missing an on/off state (use --component=Key:on|off)"
            ));
            continue;
        };
        let key = &spec[..sep];
        let val_raw = &spec[sep + 1..];
        let on = match to_lower(val_raw).as_str() {
            "on" | "true" | "1" | "yes" => true,
            "off" | "false" | "0" | "no" => false,
            other => {
                problems.push(format!(
                    "{a}: unknown state '{other}' (use on/off/true/false/1/0/yes/no)"
                ));
                continue;
            }
        };
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
            problems.push(format!(
                "unknown component key in '{a}'. Valid keys: {valid}"
            ));
        }
    }
    problems
}

/// Apply component args and merge any problems into the shared
/// unknown-args list so the execute-mode pre-mutation gate sees them.
pub fn apply_component_args_and_collect_problems(args: &[String]) {
    let problems = apply_component_args(args);
    for p in problems {
        console::err_out(&format!(
            "Invalid component selection: {p}
"
        ));
        app::opts_mut(|o| o.unknown_args.push(p));
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
    fn strict_bool_assignment_variants() {
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=off", "--preserve-nvcontainers"),
            BoolAssign::Assigned(false)
        );
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=ON", "--preserve-nvcontainers"),
            BoolAssign::Assigned(true)
        );
        assert_eq!(
            parse_bool_assignment("--PRESERVE-NVCONTAINERS=0", "--preserve-nvcontainers"),
            BoolAssign::Assigned(false)
        );
        // Bare form has no '=' assignment.
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers", "--preserve-nvcontainers"),
            BoolAssign::NotMine
        );
        // Different flag never matches.
        assert_eq!(
            parse_bool_assignment("--component=x:on", "--preserve-nvcontainers"),
            BoolAssign::NotMine
        );
        // Malformed values are now DETECTED, not silently true.
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=maybe", "--preserve-nvcontainers"),
            BoolAssign::Malformed("maybe".to_string())
        );
        assert_eq!(
            parse_bool_assignment("--preserve-nvcontainers=", "--preserve-nvcontainers"),
            BoolAssign::Malformed(String::new())
        );
    }

    #[test]
    fn ti_wait_seconds_validation_and_clamping() {
        let mut opt = Options::default();
        assert!(apply_ti_wait_seconds("90", &mut opt).is_none());
        assert_eq!(opt.ti_wait_seconds, 90);
        // Below minimum clamps up.
        assert!(apply_ti_wait_seconds("1", &mut opt).is_none());
        assert_eq!(opt.ti_wait_seconds, 15);
        // Above the task's PT2H self-limit clamps down.
        assert!(apply_ti_wait_seconds("99999999999", &mut opt).is_none());
        assert_eq!(opt.ti_wait_seconds, 7200);
        // Non-numeric is rejected outright.
        assert!(apply_ti_wait_seconds("abc", &mut opt).is_some());
    }

    #[test]
    fn flag_like_value_is_a_problem_not_a_silent_swallow() {
        // Regression: "--log-file --dry-run" used to consume the dry-run
        // switch as the log-file value, silently dropping the dry-run request
        // from what stays an execute run. The flag-like value must land in
        // unknown_args (execute mode rejects those before any mutation).
        let opts = parse_args(&["--execute".into(), "--log-file".into(), "--dry-run".into()]);
        assert!(opts.log_file_override.is_empty());
        assert_eq!(opts.unknown_args.len(), 1);
        assert!(opts.unknown_args[0].contains("--log-file"));
        assert!(opts.unknown_args[0].contains("--dry-run"));
    }

    #[test]
    fn unknown_and_malformed_args_are_collected_for_execute_rejection() {
        // Unknown option lands in unknown_args...
        let opts = parse_args(&["--dry-run".into(), "--frobnicate".into()]);
        assert_eq!(opts.unknown_args.len(), 1);
        assert!(!opts.execute);
        // ...and a malformed preserve assignment is collected too.
        let opts = parse_args(&["--dry-run".into(), "--preserve-nvcontainers=bogus".into()]);
        assert_eq!(opts.unknown_args.len(), 1);
        // execute stays false: the rejection path can never fire from this.
        assert!(!opts.execute);
    }

    #[test]
    fn mixed_case_component_flag_now_applies() {
        crate::app::init_app(Options::default());
        initialize_component_selection();
        let before = crate::app::enabled(|m| *m.get("NGX").unwrap());
        assert!(!before, "test expects NGX to start deselected");
        let arg = "--COMPONENT=ngx:on".to_string();
        let problems = apply_component_args(std::slice::from_ref(&arg));
        assert!(
            problems.is_empty(),
            "mixed-case flag must apply: {problems:?}"
        );
        let after = crate::app::enabled(|m| *m.get("NGX").unwrap());
        assert!(after, "mixed-case flag must toggle the component");
    }

    #[test]
    fn component_problems_collected_not_swallowed() {
        crate::app::init_app(Options::default());
        initialize_component_selection();
        // Missing separator.
        let p = apply_component_args(&["--component=NGX".into()]);
        assert_eq!(p.len(), 1, "missing state must be reported");
        // Garbage state.
        let p = apply_component_args(&["--component=NGX:sometimes".into()]);
        assert_eq!(p.len(), 1);
        // Unknown key lists valid keys.
        let p = apply_component_args(&["--component=Bogus:on".into()]);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("Telemetry"), "lists valid keys");
        // A well-formed call is silent.
        let p = apply_component_args(&["--component=NGX:on".into()]);
        assert!(p.is_empty());
    }
}
