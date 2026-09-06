//! Global application state, exit codes, and shared constants.
//!
//! Mirrors the C++ globals `g_options`, `g_state`, `g_componentEnabled`,
//! `g_abortRequested`, `g_userQuitRequested`. The control-handler thread only
//! ever touches the atomics; everything else is main-thread only, so plain
//! Mutexes are sufficient. Never hold one of these locks across a call into
//! another module that may lock again.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::types::{Options, RunState};

pub const RBK_VERSION: &str = "2.0.1";
pub const GPD_VERSION: &str = RBK_VERSION;

/// Scheduled-task name prefix for TrustedInstaller relaunch artifacts.
pub const K_TI_TASK_PREFIX: &str = "NvDebloatTI-";

// Exit codes (contract surface: CLI callers + parent/child handoff).
pub const EXIT_OK: i32 = 0;
pub const EXIT_FATAL_EXCEPTION: i32 = 1;
pub const EXIT_FATAL_UNKNOWN: i32 = 2;
pub const EXIT_ABORTED: i32 = 3;
pub const EXIT_TI_RELAUNCH_FAILED: i32 = 10;
pub const EXIT_TI_CHILD_FAILED: i32 = 11;
pub const EXIT_ALREADY_RUNNING: i32 = 12;
/// Invalid or unknown command-line argument rejected before any mutation.
pub const EXIT_BAD_ARGS: i32 = 13;

static OPTS: Mutex<Option<Options>> = Mutex::new(None);
static RUN: Mutex<Option<RunState>> = Mutex::new(None);
static ENABLED: Mutex<Option<BTreeMap<String, bool>>> = Mutex::new(None);
static USER_QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ctrl+C / console-close request. The console handler only stores the local
/// atomic. Ordinary execution paths calling `load` also observe the optional
/// named kernel event relayed by an unelevated launcher, so cancellation spans
/// the UAC process boundary without filesystem writes from a privileged child.
pub struct AbortFlag(AtomicBool);

impl AbortFlag {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn store(&self, value: bool, ordering: Ordering) {
        self.0.store(value, ordering);
    }

    pub fn load(&self, ordering: Ordering) -> bool {
        self.0.load(ordering) || abort_event_requested()
    }

    pub fn local_requested(&self, ordering: Ordering) -> bool {
        self.0.load(ordering)
    }
}

pub static ABORT_REQUESTED: AbortFlag = AbortFlag::new();

fn validated_abort_event(raw: &str) -> Option<&str> {
    const PREFIX: &str = "Local\\RustyButterKnife_Abort_";
    let suffix = raw.strip_prefix(PREFIX)?;
    if suffix.is_empty()
        || !suffix
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return None;
    }
    Some(raw)
}

fn abort_event_requested() -> bool {
    let event_name = {
        let g = lock(&OPTS);
        g.as_ref()
            .and_then(|o| validated_abort_event(&o.abort_event))
            .map(str::to_string)
    };
    event_name.is_some_and(|name| crate::ffi_process::named_abort_event_is_signaled(&name))
}

pub fn user_quit_requested() -> bool {
    USER_QUIT_REQUESTED.load(Ordering::SeqCst)
}

pub fn set_user_quit_requested(v: bool) {
    USER_QUIT_REQUESTED.store(v, Ordering::SeqCst);
}

pub fn init_app(opts: Options) {
    *lock(&OPTS) = Some(opts);
    *lock(&RUN) = Some(RunState::default());
    *lock(&ENABLED) = Some(BTreeMap::new());
}

/// Lock helper that recovers from poisoning: a panic while a lock is held
/// must not turn the FATAL handler's own status/report writes into a
/// secondary abort. State is main-thread-only, so recovery is safe.
fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn opts<R>(f: impl FnOnce(&Options) -> R) -> R {
    let g = lock(&OPTS);
    f(g.as_ref().expect("options initialized"))
}

/// True once init_app ran. Pause-on-exit consults this because --help/
/// --version/--list-components exit BEFORE initialization; without the
/// guard, the uniform pause rule would panic on those paths.
pub fn opts_initialized() -> bool {
    lock(&OPTS).is_some()
}

pub fn opts_mut<R>(f: impl FnOnce(&mut Options) -> R) -> R {
    let mut g = lock(&OPTS);
    f(g.as_mut().expect("options initialized"))
}

pub fn run<R>(f: impl FnOnce(&RunState) -> R) -> R {
    let g = lock(&RUN);
    f(g.as_ref().expect("run state initialized"))
}

/// Fallible variant for paths that may legitimately run before initialization
/// (the FATAL handler itself): None instead of a panic when the run state was
/// never set, so the last-resort error path cannot die of a second panic.
pub fn run_opt<R>(f: impl FnOnce(&RunState) -> R) -> Option<R> {
    let g = lock(&RUN);
    g.as_ref().map(f)
}

pub fn run_mut<R>(f: impl FnOnce(&mut RunState) -> R) -> R {
    let mut g = lock(&RUN);
    f(g.as_mut().expect("run state initialized"))
}

pub fn enabled<R>(f: impl FnOnce(&BTreeMap<String, bool>) -> R) -> R {
    let g = lock(&ENABLED);
    f(g.as_ref().expect("components initialized"))
}

pub fn enabled_mut<R>(f: impl FnOnce(&mut BTreeMap<String, bool>) -> R) -> R {
    let mut g = lock(&ENABLED);
    f(g.as_mut().expect("components initialized"))
}

/// Snapshot helper for hot paths (component-enabled lookups per scanned path).
pub fn enabled_snapshot() -> BTreeMap<String, bool> {
    enabled(|m| m.clone())
}

/// Component-enabled map type used across modules.
pub type EnabledMap = BTreeMap<String, bool>;
