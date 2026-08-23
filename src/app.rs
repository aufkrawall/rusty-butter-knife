//! Global application state, exit codes, and shared constants.
//!
//! Mirrors the C++ globals `g_options`, `g_state`, `g_componentEnabled`,
//! `g_abortRequested`, `g_userQuitRequested`. The control-handler thread only
//! ever touches the atomics; everything else is main-thread only, so plain
//! Mutexes are sufficient. Never hold one of these locks across a call into
//! another module that may lock again.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use crate::types::{Options, RunState};

pub const GPD_VERSION: &str = "1.5.0";

/// Scheduled-task name prefix for TrustedInstaller relaunch artifacts.
pub const K_TI_TASK_PREFIX: &str = "NvDebloatTI-";

// Exit codes (contract surface: PowerShell wrapper + parent/child handoff).
pub const EXIT_OK: i32 = 0;
pub const EXIT_FATAL_EXCEPTION: i32 = 1;
pub const EXIT_FATAL_UNKNOWN: i32 = 2;
pub const EXIT_ABORTED: i32 = 3;
pub const EXIT_TI_RELAUNCH_FAILED: i32 = 10;
pub const EXIT_TI_CHILD_FAILED: i32 = 11;
pub const EXIT_ALREADY_RUNNING: i32 = 12;

static OPTS: Mutex<Option<Options>> = Mutex::new(None);
static RUN: Mutex<Option<RunState>> = Mutex::new(None);
static ENABLED: Mutex<Option<BTreeMap<String, bool>>> = Mutex::new(None);
static USER_QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ctrl+C / console-close request (set from the handler thread).
pub static ABORT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn user_quit_requested() -> bool {
    USER_QUIT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn set_user_quit_requested(v: bool) {
    USER_QUIT_REQUESTED.store(v, std::sync::atomic::Ordering::SeqCst);
}

pub fn init_app(opts: Options) {
    *OPTS.lock().unwrap() = Some(opts);
    *RUN.lock().unwrap() = Some(RunState::default());
    *ENABLED.lock().unwrap() = Some(BTreeMap::new());
}

pub fn opts<R>(f: impl FnOnce(&Options) -> R) -> R {
    let g = OPTS.lock().unwrap();
    f(g.as_ref().expect("options initialized"))
}

pub fn opts_mut<R>(f: impl FnOnce(&mut Options) -> R) -> R {
    let mut g = OPTS.lock().unwrap();
    f(g.as_mut().expect("options initialized"))
}

pub fn run<R>(f: impl FnOnce(&RunState) -> R) -> R {
    let g = RUN.lock().unwrap();
    f(g.as_ref().expect("run state initialized"))
}

pub fn run_mut<R>(f: impl FnOnce(&mut RunState) -> R) -> R {
    let mut g = RUN.lock().unwrap();
    f(g.as_mut().expect("run state initialized"))
}

pub fn enabled<R>(f: impl FnOnce(&BTreeMap<String, bool>) -> R) -> R {
    let g = ENABLED.lock().unwrap();
    f(g.as_ref().expect("components initialized"))
}

pub fn enabled_mut<R>(f: impl FnOnce(&mut BTreeMap<String, bool>) -> R) -> R {
    let mut g = ENABLED.lock().unwrap();
    f(g.as_mut().expect("components initialized"))
}

/// Snapshot helper for hot paths (component-enabled lookups per scanned path).
pub fn enabled_snapshot() -> BTreeMap<String, bool> {
    enabled(|m| m.clone())
}

/// Component-enabled map type used across modules.
pub type EnabledMap = BTreeMap<String, bool>;
