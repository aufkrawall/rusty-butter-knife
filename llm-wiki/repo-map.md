# Repo Map (code map)

Last cross-checked: 2026-09-06 (v2.0.1 safety hardening)

## Top-level

- `Cargo.toml` / `Cargo.lock` — binary crate metadata and pinned dependency
  graph. Package/runtime version is mirrored by `src/app.rs::RBK_VERSION`.
- `build.py` — Windows release-build entry point. Always passes an explicit
  MSVC target triple, applies compiler path remapping, PE-verifies the requested
  architecture, and copies only verified artifacts to
  `dist/<arch>/RustyButterKnife.exe`.
- `.cargo/config.toml` — target-specific Rust/link settings.
- `.github/workflows/windows-ci-release.yml` — Windows-native gate on pushes
  and pull requests: build, clippy `-D warnings`, all targets/tests, safe CLI
  smoke. A `[release]` main push additionally builds and publishes the current
  package version as a GitHub release.
- `README.md` — user-facing behavior/CLI/safety/logging contract.
- `AGENTS.md` — development, safety and release rules.
- `assets/` — README media only.
- `llm-wiki/` — derived project memory and audit chronology.

## Rust modules (`src/`)

### Entry/orchestration

- `main.rs` — program entry, strict early-exit flow, execute-mode argument
  gate, UAC handoff, single-instance mutex, TI orchestration, status JSON and
  final pause behavior. The UAC launcher creates a named manual-reset abort
  event, forwards its name to the elevated process, signals it after local
  Ctrl+C, and remains attached until child exit.
- `app.rs` — shared application state, exit codes, version constants and
  `AbortFlag`. `AbortFlag::load` combines the local atomic with the forwarded
  named abort event; the console handler itself remains atomic-only.
- `types.rs` — `Options`, `Component`, `Candidate`, `ActionRecord`, `RunState`.
- `options.rs` — CLI parser, strict boolean/numeric parsing, component switches,
  bad-argument collection. Internal `--abort-event` is the UAC handoff only;
  it is not documented as a public flag.
- `menu.rs` — interactive component selection and `EXECUTE` confirmation.

### Safety/catalog/matching

- `components.rs` — canonical component catalog: keys, display names, defaults,
  globs and exact leaf names.
- `matching.rs` — primary fail-closed classification layer for files,
  processes, services and tasks (`ActionDecision`). DriverStore/core-display
  exclusions and component ownership live here.
- `ffi_process.rs` — **second mutation-time identity boundary** for process,
  service and task actions. New unsafe code belongs here because it wraps
  Win32 process/event handles:
  - explicit killable-process basename allowlist;
  - live `QueryFullProcessImageNameW` verification through the same handle used
    for termination, closing basename false positives and PID-reuse retargeting;
  - approved NVIDIA installation-root checks;
  - strong NVIDIA context predicate for service/task mutation;
  - named UAC abort-event create/signal/observe wrappers.
- `actions.rs` — live process/service/task operations. It consumes both the
  semantic matching layer and `ffi_process` mutation-time gates before doing
  anything destructive.

### Discovery/deletion/reporting

- `discovery.rs` — search-root construction, recursive traversal with pruning,
  candidate dedup/collapse, previous-log history and candidate-list logging.
- `fsutil.rs` — no-follow filesystem kind probes and postorder traversal.
  Reparse points are surfaces only and are never descended through.
- `deletion.rs` — candidate deletion, ownership helpers, extended-length paths,
  attribute clearing, DriverStore recursive-target guard and reboot scheduling.
- `report.rs` — cleanup orchestration after privilege setup, post-run existence
  verification and JSON report block.
- `logging.rs` — unified append/log action model and process-spanning mutex for
  large report blocks.

### Privilege/process/Task Scheduler

- `sysinfo.rs` — executable/system-directory discovery, identity/admin/TI
  checks and run-state/log-path initialization.
- `tasksched.rs` — TrustedInstaller/SYSTEM scheduled-task orchestration,
  effective child switches, bounded/abort-aware wait and child-log streaming.
- `procs.rs` — bounded external-command runner facade.
- `ffi_capture.rs` — raw Win32 subprocess capture with restricted handle
  inheritance, drain-only reads, timeout/abort termination and regression tests.
- `ffi_services.rs` — SCM enumeration, least-privilege service handles,
  bounded stop convergence.
- `ffi_tasksched.rs` — Task Scheduler COM wrappers, recursive enumeration and
  TI task registration; COM variants use RAII cleanup.
- `ffi.rs` — remaining general Win32 wrappers (console, mutexes, identity,
  Toolhelp snapshots, UAC `ShellExecuteExW`, file primitives). `unsafe` is
  confined to the `ffi*` family.

### Utilities

- `console.rs` — Unicode console/stdout/stderr and line input.
- `util.rs` — case folding, wildcard/argv quoting, CSV/JSON helpers and unique
  suffix generation.
- `winfmt.rs` — Windows timestamps/error/output formatting.

## Safety boundaries to re-check first

1. `matching.rs` — candidate/component classification and core-driver guards.
2. `ffi_process.rs` + `actions.rs` — mutation-time identity validation for
   processes/services/tasks.
3. `fsutil.rs` + `deletion.rs` — reparse/DriverStore recursive deletion rules.
4. `main.rs` + `tasksched.rs` — privilege handoff, abort semantics and parent /
   child exit-code propagation.
5. `logging.rs` — destructive runs must retain a writable audit trail.

## Verification and release

- Normal close-out gate is documented in `AGENTS.md` and mirrored in the
  Windows Actions workflow.
- Existing source is not guaranteed whole-tree rustfmt-clean; avoid unrelated
  formatting churn and keep changes scoped to touched code.
- Safe CLI smokes: `--list-components`, `--version`, `--help`; `--dry-run` only
  when candidate/report behavior needs inspection.
- Never run execute-mode regression except on a sacrificial VM.
- Release binaries are generated, not committed. A release marker is added to
  `main` only after the same tree has passed the Windows gate; the workflow then
  builds the PE-verified x86_64 artifact and creates the GitHub release.
