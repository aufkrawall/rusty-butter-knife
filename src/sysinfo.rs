//! Identity/elevation checks and run-state initialization
//! (ports of `isAdmin`, `currentTokenAccount`, `isTrustedInstaller`,
//! `getExePath`, `initializeRunState`, `systemDirFile`).

use std::path::{Path, PathBuf};

use crate::app;
use crate::console;
use crate::util::to_lower;
use crate::winfmt::now_stamp;

/// Port of `isAdmin`.
pub fn is_admin() -> bool {
    ffi_is_admin()
}

fn ffi_is_admin() -> bool {
    crate::ffi::is_admin()
}

/// Absolute path inside the real System32 directory. Invoking system tools by
/// bare name lets CreateProcess pick up a planted binary from the application
/// directory or CWD first — catastrophic in a SYSTEM-context TI child.
pub fn system_dir_file(file_name: &str) -> String {
    let Some(dir) = crate::ffi::system_directory() else {
        return file_name.to_string();
    };
    Path::new(&dir)
        .join(file_name)
        .to_string_lossy()
        .into_owned()
}

/// Port of `currentTokenAccount`: "DOMAIN\\name" or SID string fallback.
pub fn current_token_account() -> String {
    crate::ffi::current_token_account()
}

// TrustedInstaller approach:
//
// This program does NOT use undocumented APIs (NtImpersonateThread, token
// duplication, etc.) to seize TrustedInstaller's token. Instead it requests a
// scheduled task via the Task Scheduler COM API, asking the system to run as
// NT SERVICE\TrustedInstaller. Windows does not grant this request — the task
// always runs as SYSTEM — but SYSTEM owns SeBackupPrivilege and
// SeRestorePrivilege, which are sufficient to delete or take ownership of the
// NVIDIA bloat files that a normal Administrator cannot touch.
//
// The task is created by an already-elevated Administrator. It runs the same
// exe with --ti-child; after completion the parent reads LastTaskResult.
// The task is always deleted after use.
pub fn is_trusted_installer() -> bool {
    let acct = to_lower(&current_token_account());
    acct == "nt service\\trustedinstaller" || acct.contains("trustedinstaller")
}

/// Port of `getExePath`.
pub fn get_exe_path() -> PathBuf {
    crate::ffi::module_file_name()
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Port of `initializeRunState` (log path layout identical: one file per run,
/// children receive --log-file and append).
pub fn initialize_run_state() {
    let exe_path = get_exe_path();
    let (log_dir_override, log_file_override) =
        app::opts(|o| (o.log_dir_override.clone(), o.log_file_override.clone()));
    let exe_dir = if log_dir_override.is_empty() {
        exe_path.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        PathBuf::from(&log_dir_override)
    };
    let run_id = now_stamp();
    let log_path = if !log_file_override.is_empty() {
        PathBuf::from(log_file_override)
    } else {
        exe_dir.join(format!("debloat-{run_id}.log"))
    };

    let dir_to_create = if log_path.parent().is_some() {
        log_path.parent().map(Path::to_path_buf)
    } else {
        Some(exe_dir.clone())
    };
    if let Some(dir) = dir_to_create {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            console::err_out(&format!(
                "Warning: could not create log directory '{}': {}\n",
                dir.display(),
                e
            ));
        }
    }

    app::run_mut(|s| {
        s.exe_path = exe_path;
        s.exe_dir = exe_dir;
        s.run_id = run_id;
        s.log_path = log_path;
    });
}
