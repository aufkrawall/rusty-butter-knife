# Current State

Last cross-checked: 2026-08-23 (GPD_VERSION 1.5.0)

## Summary

- Native Windows C++17 NVIDIA post-install debloater; entire logic in one
  ~2500-line translation unit (`GreenPostInstallDebloatNative.cpp`).
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
- Recent activity: `log/recent.md`.

## Maintenance Notes

- `current.md` is intentionally compact. Keep detailed chronology in
  `log/recent.md` and durable subsystem rules in topical pages.
- Treat old chronology as historical unless a current topic page or source
  code anchor still confirms it.
