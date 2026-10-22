//! Log-file append, console log lines with colors, action records.

use crate::app;
use crate::console::{self, COLOR_CYAN, COLOR_RED, COLOR_WHITE, COLOR_YELLOW};
use crate::winfmt::log_time_stamp;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Named mutex serializing large multi-line appends (candidates list,
/// reports) across all processes of a run so they cannot interleave in the
/// single shared log.
pub const K_LOG_MUTEX_NAME: &str = "Global\\RustyButterKnife_LogMutex";

static APPEND_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn report_append_failure_once(err: &std::io::Error, path: &std::path::Path) {
    // Escape hatch against recursion: log_line reporting its own append
    // failure must not trigger another failed report.
    if APPEND_FAILURE_REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    console::err_out(&format!(
        "ERROR: cannot write to run log {}: {}\n",
        path.display(),
        err
    ));
}

/// Port of `appendUtf8File` (append/create semantics), now fallible so
/// losing the audit trail is observable instead of silent.
pub fn append_utf8_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    if path.as_os_str().is_empty() {
        return Ok(()); // matches CreateFileW("") failing silently pre-init
    }
    let mut f = OpenOptions::new().append(true).create(true).open(path)?;
    f.write_all(text.as_bytes())
}

/// Append a LARGE multi-line section while holding a named process-spanning
/// mutex so concurrent launcher/elevated/SYSTEM writers cannot interleave
/// mid-section. Falls back to a direct append if the mutex is unavailable
/// for a bounded grace; append errors are surfaced exactly once.
pub fn append_block_serialized(path: &std::path::Path, text: &str) {
    const WAIT_GRACE_MS: u32 = 5_000;
    let acquired = crate::ffi::try_acquire_named_mutex(K_LOG_MUTEX_NAME, WAIT_GRACE_MS);
    let result = append_utf8_file(path, text);
    drop(acquired); // releases if it was acquired
    if let Err(e) = result {
        report_append_failure_once(&e, path);
    }
}

/// File-only level: diagnostic detail kept in the run log but never shown on
/// the console (also skipped when a parent streams a worker's log lines).
pub const DETAIL: &str = "DETAIL";
/// Section heading: rendered as a `== text ==` heading on the console.
pub const STEP: &str = "STEP";

/// WARN/ERROR/FATAL lines logged by this process (for the closing summary).
static PROBLEMS_LOGGED: AtomicUsize = AtomicUsize::new(0);

pub fn problems_logged() -> usize {
    PROBLEMS_LOGGED.load(Ordering::SeqCst)
}

pub(crate) fn color_for_level(level: &str) -> u16 {
    match level {
        "ERROR" | "FATAL" => COLOR_RED,
        "WARN" => COLOR_YELLOW,
        "DRYRUN" | STEP => COLOR_CYAN,
        _ => COLOR_WHITE,
    }
}

/// Human console rendering of one log record; None = file-only. The log
/// file keeps the full `[date time] [LEVEL] message` form for tools.
pub fn render_console(level: &str, timestamp: &str, message: &str) -> Option<String> {
    let clock = timestamp.rsplit(' ').next().unwrap_or(timestamp);
    match level {
        DETAIL => None,
        STEP => Some(format!("\n== {message} ==\n")),
        "WARN" => Some(format!("{clock}  WARNING: {message}\n")),
        "ERROR" | "FATAL" => Some(format!("{clock}  {level}: {message}\n")),
        _ => Some(format!("{clock}  {message}\n")),
    }
}

/// Split a run-log line `[timestamp] [LEVEL] message` into its parts. None
/// for untimestamped lines, which only occur inside multi-line blocks
/// (candidate table, post-run check, JSON report).
pub fn parse_log_record(line: &str) -> Option<(&str, &str, &str)> {
    let rest = line.strip_prefix('[')?;
    let (ts, rest) = rest.split_once("] [")?;
    let (level, message) = rest.split_once("] ")?;
    Some((ts, level, message))
}

/// Log messages stay single-line: an embedded line break would create an
/// untimestamped line that readers treat as block content.
fn one_line(message: &str) -> String {
    message.replace("\r\n", " | ").replace(['\r', '\n'], " | ")
}

/// Human wording for an action record: `<what happened> [component]: path
/// (detail)`. The JSON report keeps the raw kind/status/detail fields.
pub fn action_line(kind: &str, status: &str, component: &str, path: &str, detail: &str) -> String {
    let label = match (kind, status) {
        ("DeletePath", "DRYRUN") => "Would delete",
        ("DeletePath", "SKIP") => "Skipped",
        ("DeletePath", "WARN") => "Could not delete",
        ("DeletePath", _) => "Deleted",
        ("ScheduleDelete", "WARN") => "Could not queue for deletion at restart",
        ("ScheduleDelete", _) => "Queued for deletion at restart",
        ("KillProcess", "DRYRUN") => "Would stop process",
        ("KillProcess", "SKIP") => "Left process running",
        ("KillProcess", "WARN") => "Could not stop process",
        ("KillProcess", _) => "Stopped process",
        ("StopService", "DRYRUN") => "Would stop service",
        ("StopService", "WARN") => "Could not stop service",
        ("StopService", _) if detail == "Stopped" => "Stopped service",
        ("StopService", _) => "Service was not running",
        ("DisableService", "DRYRUN") => "Would disable service",
        ("DisableService", "WARN") => "Could not disable service",
        ("DisableService", _) => "Disabled service",
        ("DeleteService", "DRYRUN") => "Would delete service",
        ("DeleteService", "WARN") => "Could not delete service",
        ("DeleteService", _) => "Deleted service",
        ("Service", "SKIP") => "Left service unchanged",
        ("Service", _) => "Could not open service",
        ("DeleteTask", "DRYRUN") => "Would delete scheduled task",
        ("DeleteTask", "WARN") => "Could not delete scheduled task",
        ("DeleteTask", _) => "Deleted scheduled task",
        ("DisableTask", "DRYRUN") => "Would disable scheduled task",
        ("DisableTask", "WARN") => "Could not disable scheduled task",
        ("DisableTask", _) => "Disabled scheduled task",
        ("ScheduledTask", "SKIP") => "Left scheduled task unchanged",
        _ => {
            let mut msg = format!("{kind} [{component}] {path}");
            if !detail.is_empty() {
                msg.push_str(" :: ");
                msg.push_str(detail);
            }
            return msg;
        }
    };
    let mut msg = label.to_string();
    if !component.is_empty() && component != "RebootDelete" {
        msg.push_str(&format!(" [{component}]"));
    }
    msg.push_str(&format!(": {path}"));
    let detail = human_detail(detail);
    if !detail.is_empty() {
        msg.push_str(&format!(" ({detail})"));
    }
    msg
}

/// Drop details the label already says; translate terse internal ones.
fn human_detail(detail: &str) -> String {
    match detail {
        "" | "file" | "Deleted file" | "Stopped" | "Already stopped" | "Not running"
        | "Disabled" | "Deleted" | "Pending delete at reboot" | "Terminate requested" => {
            String::new()
        }
        "directory" => "folder".to_string(),
        "component deselected" => "component off".to_string(),
        "Unsafe recursive directory target" => {
            "protected driver folder, never deleted as a whole".to_string()
        }
        d => match d.strip_prefix("Removed entries=") {
            Some(n) => format!("folder, {n} entries"),
            None => d.to_string(),
        },
    }
}

pub(crate) fn no_color() -> bool {
    // Pre-init FATAL lines (run state not yet set up) cannot know the flag;
    // default to color-off there instead of panicking in the error path.
    if !app::opts_initialized() {
        return true;
    }
    app::opts(|o| o.no_color)
}

/// Print one record on the console in its human form (nothing for DETAIL).
pub(crate) fn print_console_record(level: &str, timestamp: &str, message: &str) {
    let Some(text) = render_console(level, timestamp, message) else {
        return;
    };
    let colored = !no_color();
    console::set_color(color_for_level(level), colored);
    console::out(&text);
    console::set_color(COLOR_WHITE, colored);
}

/// Port of `logLine`: human console line (see `render_console`) plus the
/// full `[timestamp] [LEVEL] message` UTF-8 append to the single run log.
/// Append failures are surfaced on stderr exactly once per run instead of
/// being swallowed.
pub fn log_line(level: &str, message: &str) {
    let message = one_line(message);
    let timestamp = log_time_stamp();
    let line = format!("[{timestamp}] [{level}] {message}\n");
    if matches!(level, "WARN" | "ERROR" | "FATAL") {
        PROBLEMS_LOGGED.fetch_add(1, Ordering::SeqCst);
    }
    print_console_record(level, &timestamp, &message);
    // Pre-init FATAL lines have no run log yet; skip the append instead of
    // panicking inside the FATAL handler.
    if let Some(log_path) = app::run_opt(|s| s.log_path.clone()) {
        if let Err(e) = append_utf8_file(&log_path, &line) {
            report_append_failure_once(&e, &log_path);
        }
    }
}

/// Port of `addAction`.
pub fn add_action(kind: &str, status: &str, component: &str, path: &str, detail: &str) {
    app::run_mut(|s| {
        s.actions.push(crate::types::ActionRecord {
            kind: kind.to_string(),
            status: status.to_string(),
            component: component.to_string(),
            path: path.to_string(),
            detail: detail.to_string(),
        });
    });
}

/// Port of `logAction`: records the action and logs its human wording at
/// the action's status level.
pub fn log_action(kind: &str, status: &str, component: &str, path: &str, detail: &str) {
    add_action(kind, status, component, path, detail);
    log_line(status, &action_line(kind, status, component, path, detail));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn console_rendering_is_human_and_detail_is_file_only() {
        let ts = "2026-10-22 11:59:31";
        assert_eq!(render_console(DETAIL, ts, "RunId: x"), None);
        assert_eq!(render_console("INFO", ts, "hi").unwrap(), "11:59:31  hi\n");
        assert_eq!(
            render_console("WARN", ts, "careful").unwrap(),
            "11:59:31  WARNING: careful\n"
        );
        assert_eq!(render_console(STEP, ts, "Result").unwrap(), "\n== Result ==\n");
    }

    #[test]
    fn log_messages_stay_single_line() {
        assert_eq!(one_line("a\r\nb\nc"), "a | b | c");
    }

    #[test]
    fn action_lines_read_as_sentences() {
        assert_eq!(
            action_line("DeletePath", "INFO", "Telemetry", "C:\\x.dll", "Deleted file"),
            "Deleted [Telemetry]: C:\\x.dll"
        );
        assert_eq!(
            action_line("DeletePath", "INFO", "Installer2Cache", "C:\\I2", "Removed entries=168"),
            "Deleted [Installer2Cache]: C:\\I2 (folder, 168 entries)"
        );
        assert_eq!(
            action_line("DeletePath", "DRYRUN", "Telemetry", "C:\\d", "directory"),
            "Would delete [Telemetry]: C:\\d (folder)"
        );
        assert_eq!(
            action_line("ScheduleDelete", "INFO", "RebootDelete", "C:\\f", "Pending delete at reboot"),
            "Queued for deletion at restart: C:\\f"
        );
        assert_eq!(
            action_line("Service", "SKIP", "FrameView", "Svc (Svc)", "component deselected"),
            "Left service unchanged [FrameView]: Svc (Svc) (component off)"
        );
        assert_eq!(
            action_line("StopService", "INFO", "Telemetry", "S", "Not running"),
            "Service was not running [Telemetry]: S"
        );
        // Unknown kinds keep the raw form rather than inventing wording.
        assert_eq!(
            action_line("Custom", "INFO", "C", "p", "d"),
            "Custom [C] p :: d"
        );
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!(
                "gpd-log-{tag}-{}-{}",
                std::process::id(),
                crate::util::now_unique_suffix()
            ));
            std::fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn append_reports_io_errors_instead_of_silence() {
        // Appending to a DIRECTORY must return Err, never vanish silently.
        let t = TempDir::new("err");
        assert!(append_utf8_file(&t.0, "x").is_err());
    }

    #[test]
    fn append_creates_and_extends_log() {
        let t = TempDir::new("ok");
        let log = t.0.join("debloat-test.log");
        append_utf8_file(&log, "one\n").expect("first append");
        append_utf8_file(&log, "two\n").expect("second append");
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text, "one\ntwo\n");
        // Serialized block variant succeeds on the same file.
        append_block_serialized(&log, "block\n");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.ends_with("block\n"));
    }
}
