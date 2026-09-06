<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 aufkrawall
-->

# Agent Instructions

## Critical workflow

- **Platform/toolchain baseline:** Windows 10/11 with stable Rust (MSVC) and
  Python 3. Primary builds are `cargo build --release` and `python build.py`.
- **DANGER — destructive system tool.** During development/verification use
  only safe invocations such as `--dry-run`, `--list-components`, `--help`,
  and `--version`. Never run `--execute`, `--kill-lockers`, or a bare launch
  under an agent harness. Execute-mode regression belongs only on a sacrificial
  VM with rollback available.
- **Development loop:** stay in `cargo build` while iterating. Close substantial
  changes with one gate:
  1. `cargo build`
  2. `cargo clippy --all-targets -- -D warnings`
  3. `cargo test --all-targets`
  4. when behavior/CLI changed, one safe smoke such as
     `./target/debug/RustyButterKnife.exe --list-components`, `--version`, or
     `--help`; use `--dry-run` only when candidate/report behavior needs review.
- `.github/workflows/windows-ci-release.yml` mirrors this gate on Windows for
  pushes and pull requests. CI does **not** replace sacrificial-VM execute-mode
  testing, sanitizers, or fuzzing; those remain coverage gaps.
- Existing source is not guaranteed whole-tree rustfmt-clean. Do not reformat
  unrelated files merely to impose formatter output; keep edits scoped to the
  task and surrounding style.
- CLI/help changes must be checked against the README flag table.
- Fix every warning introduced by a change. Warning-free build/clippy is part
  of the contract.
- Built binaries, archives, generated logs, `target/`, `dist/`, and other
  generated artifacts are not committed. `build.py` PE-verifies release
  architecture before copying into `dist/<arch>/`.
- Version bumps must update both `Cargo.toml` and `src/app.rs` (`RBK_VERSION`),
  with `Cargo.lock` kept consistent. The guarded release workflow publishes a
  verified x86_64 executable only from a `main` push whose head commit message
  contains `[release]`; do not use that marker until the same tree has passed
  the Windows gate.
- Always commit code changes. Do not push unless explicitly requested.
- Keep `llm-wiki/` current when durable project knowledge changes.
- Mistrust source comments, docs and wiki equally; verify claims against the
  current code and observed tests/builds.

## Engineering rules

- Prefer root-cause fixes over workarounds. Do not hide, weaken, or paper over
  failures.
- Do not introduce timing bandaids, racy behavior, or unbounded waits.
- **Fail closed on destructive targeting.** Matching/classification and
  mutation-time identity checks are safety boundaries. When a target cannot be
  confidently identified as intended NVIDIA bloat, do not mutate it.
- Whole DriverStore package roots are undeletable; recursive deletion inside
  DriverStore is allow-listed; traversal never crosses reparse points.
- Process termination has an additional invariant: only explicit known process
  basenames qualify, and the live full executable path must be verified through
  the same handle used for termination (`src/ffi_process.rs`). Do not weaken
  this into generic substring matching.
- Service/task mutation requires a strong NVIDIA context before generic terms
  such as `update`, `share`, or `broadcast` are considered.
- `unsafe` belongs only in the `ffi*` module family, behind safe wrappers with
  ownership/invariant comments. The crate root denies unsafe elsewhere.
- Source-code files should stay roughly 500–800 lines. Split along meaningful
  subsystem boundaries rather than allowing monoliths to grow.
- Treat logs, dumps, media, credentials, tokens, private keys and user data as
  sensitive. Never commit them.

## Build, diagnostics, and tests

- Current verification consists of Windows-native build/clippy/tests plus safe
  CLI smoke. The crate includes unit tests and real-Windows integration tests
  for bounded process capture, junction/reparse behavior, task/service
  decisions, CLI parsing, reporting, mutation-target safety and cross-process
  abort-event signaling.
- No sanitizer/fuzzer stage exists yet. Destructive Win32 call sites still need
  a disposable VM harness for true execute-mode regression.
- When matching behavior changes, add negative tests for false positives as
  well as positive tests for intended NVIDIA targets. Dry-run candidate logs can
  supplement tests but must not substitute for decision-level regression tests.
- `build.py` intentionally aborts on non-Windows hosts. It always passes the
  requested target triple explicitly and PE-verifies the result.

## Debugging and logging

- Add high-signal, rate-limited logging when it materially helps diagnose state
  transitions or failures. Route runtime records through `log_line` /
  `log_action` so launcher, elevated instance and SYSTEM worker share one log.
- A destructive run must establish a writable audit log before mutation.
- Cross-process Ctrl+C during UAC handoff uses a manual-reset named Windows
  event. The launcher remains attached until the elevated process actually
  exits; never reintroduce a path that reports exit 3 while elevated destructive
  work can continue.
- External tools (`schtasks.exe`, `takeown.exe`, `icacls.exe`) must be resolved
  from the real `%SystemRoot%\System32`, never via PATH/CWD in privileged code.

## Useful commands and paths

| Tool | Purpose |
| --- | --- |
| `RustyButterKnife.exe --dry-run` | Safe scan + unified report/log; no mutations |
| `RustyButterKnife.exe --list-components` | Fast component/default-state sanity check |
| `RustyButterKnife.exe --version` / `--help` | Safe CLI/version smoke |
| `debloat-*.log` | Unified run log; override with `--log-file` / `--log-dir` |
| `python build.py --arch x86_64` | Verified release build into `dist/x86_64/` |
| `python build.py --arch aarch64` | Verified ARM64 cross-build into `dist/aarch64/` |

## `llm-wiki/` workflow

- `llm-wiki/` is canonical LLM-maintained derived memory, not the sole source
  of truth.
- For substantial unfamiliar work, orient via `llm-wiki/index.md`, then
  `repo-map.md`, the relevant topical pages, and `log/recent.md` for chronology.
- Prefer updating existing pages over creating new ones. Put durable current
  rules in topical pages/current state; put chronology in `log/recent.md`.
- Mark uncertainty explicitly. Do not preserve stale claims for historical
  sentiment when source code disproves them.
- After code + wiki changes, check for contradictions, stale counts/workflows,
  broken links, duplicate claims and orphaned pages.
