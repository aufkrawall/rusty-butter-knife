# Current State

Last cross-checked: 2026-08-27 (audit pass 3: root-reparse fix + abort-aware
TI wait; GPD_VERSION 1.5.0)

## Summary

- Native Windows NVIDIA post-install debloater. PRIMARY: Rust crate at repo
  root (`src/`, 51 unit/integration tests, clippy-clean, unsafe confined to
  the `ffi*` module family). LEGACY: C++17 single TU kept as REFERENCE code;
  `python build.py` builds only the Rust leg by default (`--variant cpp|all`
  builds it); both legs are PE-machine-type verified per architecture.
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
