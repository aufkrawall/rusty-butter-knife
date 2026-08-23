# Current State

Last cross-checked: 2026-08-23 (GPD_VERSION 1.5.0)

## Summary

- Native Windows NVIDIA post-install debloater. PRIMARY: Rust crate at repo
  root (`src/`, modules, clippy-clean, unsafe confined to `ffi*` modules).
  LEGACY: C++17 single TU kept as reference, still builds via `python build.py`.
- Built via `python build.py` (pinned llvm-mingw in `mingw64/`, x86_64
  default, aarch64 cross-target available). No test suite, no CI.
- Destructive system tool: dry-run by default; destructive execution needs
  TrustedInstaller relaunch (scheduled-task COM) or explicit fallback flags,
  interactive EXECUTE confirmation, and holds a single-instance mutex.
- Agents must only run safe flags (`--dry-run`, `--list-components`,
  `--help`); see AGENTS.md gate definition.

## Routing

- Build/toolchain: `repo-map.md` (build pipeline section), `README.md`
  (Build section).
- Repo layout / source section map: `repo-map.md`.
- Safety predicates and deletion guards: `repo-map.md` matching/predicate
  layer + AGENTS.md "Safety-predicate discipline".
- Style/conventions: `codestyle.md`.
- Verification/diagnostics: `debug-tools.md`.
- Accepted trade-offs (no tests, arch output collision, single TU):
  `known-debt.md`.
- Rust port feasibility: `rust-port-feasibility.md`.
- Recent activity: `log/recent.md`.

## Maintenance Notes

- `current.md` is intentionally compact. Keep detailed chronology in
  `log/recent.md` and durable subsystem rules in topical pages.
- Treat old chronology as historical unless a current topic page or source
  code anchor still confirms it.
