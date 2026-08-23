# Code Style

Last cross-checked: 2026-08-23

Primary sources:
- `AGENTS.md`
- `GreenPostInstallDebloatNative.cpp` (representative: `toLower`, `logLine`,
  `buildComponents`, `wmain`)
- `build.py`

## Scope
This page records the style rules that are strongly reflected in the current
tree. There is NO formatter or linter config in this repo — local file
conventions win.

## C++ (`GreenPostInstallDebloatNative.cpp`)

- Standard: C++17, compiled with `-std=c++17 -municode -O2 -Wall -Wextra
  -static`. New warnings are gate failures; fix them, don't suppress.
- Unicode throughout: `UNICODE`/`_UNICODE` defined, `std::wstring` and
  `L""` literals everywhere, `wmain` entry point. Narrow strings only at
  process-output boundaries (see `widen`, `decodeProcessOutput`).
- 4-space indentation, LF line endings (git `core.autocrlf=input`), UTF-8,
  K&R brace style (opening brace on same line), no tabs.
- Naming:
  - functions / locals: `camelCase` (`discoverCandidates`,
    `killLockerProcesses`)
  - types/structs/enums: `PascalCase` (`Candidate`, `RunState`,
    `TiRelaunchResult`)
  - constants: `kCamelCase` (`kTiTaskPrefix`) or `EXIT_*` SCREAMING_CASE for
    exit codes
  - globals: `g_` prefix (`g_options`, `g_state`, `g_componentEnabled`)
- Comment density: section-divider comments between major areas; explanatory
  comments where Win32 semantics are non-obvious. Keep that pattern.
- All console/log output flows through `logLine()`/`logAction()`; user-
  facing strings are English wide strings. Never `printf`/`std::cout`
  directly for run events.
- System executables are invoked by absolute path from `%SystemRoot%\
System32` (`systemDirFile`), never via PATH search. Preserve this.
- Prefer fail-closed predicates in matching code: when uncertain whether a
  path is bloat, return "not a candidate".

## Python (`build.py`)

- Stdlib only; no third-party dependencies. argparse CLI, module-level
  constants in UPPER_CASE, snake_case functions.
- Progress lines use `[*]` / warnings `[!]` prefixes; keep that format.

## Common Tree Conventions

- Version lives ONLY in `GPD_VERSION` (top of the .cpp) — bump it there and
  keep README + header comment consistent when behavior changes.
- README flag table ↔ `printUsage()` ↔ header comment must agree; update
  all three when flags change.

## Practical Notes
- Do not run a whole-file automatic formatter on existing source unless
  explicitly requested; it would produce a huge unrelated diff on a
  ~2500-line file.
- Preserve the touched file's existing formatting and line endings. Inspect
  the diff before building.
- If formatter output, local file style, and this page disagree, preserve
  the local file's established pattern unless the user explicitly requested
  a formatting migration.

### Current lint debt and triage

No lint ratchet configured yet. The compiler's `-Wall -Wextra` is the only
static analysis; the working tree is currently warning-clean at the pinned
llvm-mingw version. Treat any new warning as a regression to fix now rather
than debt to record.
