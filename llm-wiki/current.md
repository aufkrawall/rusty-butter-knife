# Current State

Last cross-checked: 2026-09-05 (rename to Rusty Butter Knife, dropped C++, v2.0.0)

## Summary

- Native Windows NVIDIA driver post-install debloater. Pure Rust crate at repo
  root (`src/`, 57 unit/integration tests, clippy-clean, unsafe confined to
  the `ffi*` module family). C++ legacy version dropped completely.
  `python build.py` or `cargo build --release` builds `RustyButterKnife.exe`
  with PE-machine verification and compiler path remapping (`--remap-path-prefix`)
  to guarantee zero build-path or username leakage.
- Destructive system tool: dry-run by default; destructive execution needs
  TrustedInstaller relaunch (scheduled-task COM) or explicit fallback flags,
  interactive EXECUTE confirmation, a single-instance mutex, and a writable
  audit log (probed before any destructive stage).
- Safety model (audit-hardened, see `log/recent.md` entry
  "2026-08-26 audit remediation"):
  - EVERY mutation (file/process/service/task) routes through
    `matching::ActionDecision` — deselected components block ALL their
    actions; unclassified targets fail closed.
  - Subprocess capture is hard-bounded (`ffi_capture.rs`): drain-only reads,
    terminate + grace on timeout, abort-aware waits; regression tests exist.
  - No recursion through reparse points (`fsutil.rs` PathKind boundary).
  - Services use least-privilege masks with exact-mask tests and bounded
    stop-convergence via QueryServiceStatusEx.
  - Execute mode rejects malformed/unknown CLI input BEFORE mutation
    (exit code 13).

## Routing

- Build/toolchain: `repo-map.md` (build pipeline section), `README.md`
  (Build section).
- Repo layout / module map: `repo-map.md`.
- Safety predicates + classification layer:
  `repo-map.md` matching/predicate layer + AGENTS.md "Safety-predicate
  discipline".
- Style/conventions: `codestyle.md`.
- Verification/diagnostics: `debug-tools.md`.
- Accepted trade-offs and DEFERRED fixes (takeown native APIs etc.):
  `known-debt.md`.
- Recent activity incl. stale-claim corrections: `log/recent.md`.

## Maintenance Notes

- `current.md` is intentionally compact. Keep detailed chronology in
  `log/recent.md` and durable subsystem rules in topical pages.
- Treat old chronology as historical unless a current topic page or source
  code anchor still confirms it. Notably, several 2026-08-23 entries
  overclaimed fixes that were only truly landed on 2026-08-26.
