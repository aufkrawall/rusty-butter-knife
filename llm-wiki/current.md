# Current State

Last cross-checked: 2026-09-06 (full-repo audit pass; CI least privilege, wiki drift repair — see log/recent.md)

## Summary

- Native Windows NVIDIA driver post-install debloater. Pure Rust crate at
  repo root (`src/`, 64 unit/integration tests, unsafe confined to the
  `ffi*` module family). C++ legacy version is gone.
  `python build.py` or `cargo build --release` builds `RustyButterKnife.exe`
  with explicit target triples, PE-machine verification and compiler path
  remapping (`--remap-path-prefix`).
- Destructive system tool: dry-run by default; destructive execution needs
  TrustedInstaller relaunch (scheduled-task COM) or explicit fallback flags,
  interactive EXECUTE confirmation, a single-instance mutex, and a writable
  audit log (probed before any destructive stage).
- Safety model:
  - EVERY mutation (file/process/service/task) routes through component
    classification; deselected components block their actions and unknown
    targets fail closed.
  - Process termination has a second mutation-time gate in `ffi_process.rs`:
    only explicit known basenames qualify, and the full live image path is
    queried through the same handle used for termination. This closes generic
    basename false positives and PID-reuse retargeting.
  - Service/task mutations require a stronger NVIDIA anchor before broad
    semantic terms such as update/share/broadcast can classify a target.
  - UAC cancellation uses a named manual-reset Windows event. The unelevated
    launcher signals it after Ctrl+C and remains attached until the elevated
    child actually exits; it cannot report exit 3 while destructive child work
    continues, and privileged cancellation does not require filesystem writes.
  - Subprocess capture is hard-bounded (`ffi_capture.rs`), traversal never
    crosses reparse points (`fsutil.rs`), and service access is least-privilege
    with bounded stop convergence.
  - Execute mode rejects malformed/unknown CLI input before mutation; numeric
    TI wait values require full-string integer parsing.
- `.github/workflows/windows-ci-release.yml` provides a Windows-native gate on
  pushes/PRs: build, clippy `-D warnings`, all tests and safe CLI smokes. A
  marked merge commit on `main` (`[release]`) builds the verified x86_64
  executable and publishes the versioned GitHub release.
- Existing source is not guaranteed whole-tree rustfmt-clean; verification
  intentionally avoids unrelated formatter churn and follows the scoped style
  policy in `AGENTS.md`.

## Routing

- Build/toolchain: `repo-map.md` and README Build section.
- Repo layout / module map: `repo-map.md`.
- Safety predicates + mutation target verification: `matching.rs`,
  `ffi_process.rs`, `actions.rs`, plus AGENTS.md safety-predicate discipline.
- Style/conventions: `codestyle.md`.
- Verification/diagnostics: `debug-tools.md` and the Windows Actions workflow.
- Accepted trade-offs and DEFERRED fixes: `known-debt.md`.
- Recent activity and release chronology: `log/recent.md`.

## Maintenance Notes

- `current.md` is intentionally compact. Keep detailed chronology in
  `log/recent.md` and durable subsystem rules in topical pages.
- Treat old chronology as historical unless a current topic page or source
  anchor still confirms it.
