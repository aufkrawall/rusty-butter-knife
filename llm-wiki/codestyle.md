# Code Style

Last cross-checked: 2026-09-06 (rewritten for the Rust-only tree; the legacy
C++ translation unit was removed in v2.0.0)

Primary sources:
- `AGENTS.md`
- `src/` (the entire implementation)
- `build.py`

## Scope
This page records the style rules that are strongly reflected in the current
tree. There is NO formatter or linter config in this repo — local file
conventions win.

## Rust (`src/`, sole implementation)

- Toolchain: stable MSVC Rust, edition 2021. The close-out gate is
  `cargo build` + `cargo clippy --all-targets -- -D warnings` +
  `cargo test --all-targets` + one safe CLI smoke (see `AGENTS.md`). There is
  deliberately NO `cargo fmt` gate: existing source is not guaranteed
  whole-tree rustfmt-clean, and verification avoids unrelated formatter churn.
- **Unsafe policy:** crate root has `#![deny(unsafe_code)]`; unsafe exists
  ONLY inside the `ffi*` module family (`ffi.rs`, `ffi_capture.rs`,
  `ffi_process.rs`, `ffi_services.rs`, `ffi_tasksched.rs`), each call wrapped
  safe with a SAFETY comment. Verified by grep: zero unsafe blocks anywhere
  else.
- **File-size discipline:** every source file under ~800 lines (largest:
  matching.rs ~640). Split along subsystem boundaries when approaching it.
- Modules mirror the legacy C++ section map (see repo-map.md); snake_case
  files and functions (`log_line` corresponds to legacy C++ `logLine`).
  Types/structs/enums are PascalCase, constants SCREAMING_CASE for exit codes
  and `K_`/upper prefixes elsewhere.
- Comment density: section-divider comments between major areas; explanatory
  comments where Win32 semantics are non-obvious, especially SAFETY comments
  at every unsafe boundary. Keep that pattern.
- All console/log output flows through `log_line()`/`log_action()` (console
  bytes via `console::out`/`console::err_out`); user-facing strings are
  English. Never write to stdout/stderr directly for run events.
- System executables are invoked by absolute path from `%SystemRoot%\
System32` (`sysinfo::system_dir_file`), never via PATH search. Preserve this.
- Prefer fail-closed predicates in matching code: when uncertain whether a
  path is bloat, return "not a candidate".
- Contracts (do not drift): exit codes (`app::EXIT_*`), status-file JSON keys,
  log markers ("run header ====", "Candidate count: "), flag surface,
  component catalog strings (`components.rs`). `--help`/`print_usage` text and
  the README flag table must stay in sync.
- Dependencies: only Microsoft-published crates (`windows`, `windows-sys`),
  pinned via committed Cargo.lock. No other crates without user approval.
  All declared `windows-sys` features are load-bearing except
  `Win32_UI_WindowsAndMessaging` (removed 2026-09-06 after a clean
  no-feature compile check); `Win32_System_Registry` IS required (it gates
  `ShellExecuteExW` in windows-sys 0.60 despite the Shell import).

## Python (`build.py`)

- Stdlib only; no third-party dependencies. argparse CLI, module-level
  constants in UPPER_CASE, snake_case functions.
- Progress lines use `[*]` / warnings `[!]` prefixes; keep that format.

## Common Tree Conventions

- Version lives in `Cargo.toml` AND `src/app.rs` (`RBK_VERSION`) — bump both
  and keep `Cargo.lock` consistent; README flags/exit codes must agree with
  behavior when it changes.

## Practical Notes
- Do not run a whole-tree automatic formatter; keep edits scoped to touched
  code and preserve the touched file's existing formatting and line endings.
  Inspect the diff before building.
- If formatter output, local file style, and this page disagree, preserve
  the local file's established pattern unless the user explicitly requested
  a formatting migration.

### Current lint debt and triage

No lint ratchet configured yet. `cargo clippy --all-targets -- -D warnings`
is the standing static-analysis gate; the tree is warning-clean. Treat any
new warning as a regression to fix now rather than debt to record.
