//! External process execution with output capture
//! (ports of `ProcessResult`, `runProcessCapture`, `runShellCommand`).

use crate::app;
use crate::logging::log_line;
use crate::util::trim;
use crate::winfmt::decode_process_output;

#[derive(Clone)]
pub struct ProcessResult {
    pub exit_code: u32,
    pub output: String,
    /// Parity with the legacy struct; consumers only read exit/output.
    #[allow(dead_code)]
    pub started: bool,
}

impl Default for ProcessResult {
    fn default() -> Self {
        ProcessResult {
            exit_code: 0xFFFF_FFFF,
            output: String::new(),
            started: false,
        }
    }
}

/// Port of `runProcessCapture` (120 s default timeout lives at call sites).
pub fn run_process_capture(command_line: &str, timeout_ms: u32) -> ProcessResult {
    let res = crate::ffi::capture_process(command_line, timeout_ms, decode_process_output);
    ProcessResult {
        exit_code: res.exit_code,
        output: res.output,
        started: res.started,
    }
}

/// Port of `runShellCommand`: dry-run logging plus INFO/WARN result line.
pub fn run_shell_command(command_line: &str, label: &str, dry_run_ok: bool) -> bool {
    let is_execute = app::opts(|o| o.execute);
    if !is_execute && dry_run_ok {
        log_line("DRYRUN", &format!("Would run: {command_line}"));
        return true;
    }
    let res = run_process_capture(command_line, 120_000);
    let mut msg = format!("{label} exit={}", res.exit_code);
    let trimmed = trim(&res.output);
    if !trimmed.is_empty() {
        msg.push_str(&format!(" output={trimmed}"));
    }
    log_line(if res.exit_code == 0 { "INFO" } else { "WARN" }, &msg);
    res.exit_code == 0
}
