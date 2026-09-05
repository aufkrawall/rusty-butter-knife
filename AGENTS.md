<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 aufkrawall
-->

# Agent Instructions

## Critical workflow

- **Platform/toolchain baseline:** Windows 10/11 host with Rust (MSVC toolchain)
  and Python 3 available. The primary build is `cargo build --release` or
  `python build.py`. Shell is PowerShell/cmd; git is configured with LF line
  endings (`core.autocrlf=input`).
- **DANGER — this is a destructive system tool.** The program deletes files,
  disables/deletes services and scheduled tasks on real NVIDIA installations.
  During development/verification, ONLY ever run it with safe flags:
  `--dry-run`, `--list-components`, `--help`. Never invoke `--execute`,
  `--kill-lockers`, or a bare launch (which opens the
  wizard preselected for destructive mode).
- **Default development loop:** Pure **Rust crate** at the repo root —
  its loop is `cargo build` (single-digit seconds). Stay in this loop while
  iterating; do not run extra verification after every small edit.
- **Then close with exactly ONE gate.**
  - Rust: `cargo build` + `cargo clippy --all-targets -- -D warnings`
    + `cargo test --all-targets`, zero failures. When behavior changed, one
    safe smoke run:
    `./target/release/RustyButterKnife.exe --list-components`
    and/or `--dry-run` under a timeout (writes a gitignored log beside the
    binary).
  - Change touching CLI flags/help text -> additionally diff `--help` output
    against the flag table in `README.md` and update the README if they drift.
- What the gate does NOT cover: there is no CI, no sanitizers, and execute-
  mode regression must run only on sacrificial VMs. `cargo test --all-targets`
  is part of the gate (51+ tests incl. real-Windows integration tests); compiler
  and clippy cleanliness remain the first-line regression net.
- Fix any new warning introduced by a change; warning-free code is part of
  the gate.
- No fuzzing stage exists.
- Release process: built binaries (`*.exe`), archives (`*.7z`) and logs are
  deliberately NOT committed (see `.gitignore`). Bump version in `Cargo.toml`
  and `RBK_VERSION` in `src/app.rs` when the user asks for a version bump.
- Prefer explicit flags over interactive runs for agent sessions. A bare
  launch blocks forever waiting on wizard input — under an agent harness that
  is a hang; always pass flags and use timeouts.
- Always commit after code changes!
- Match the surrounding code's existing indentation, naming, comment density,
  and line endings; keep edits narrowly scoped and inspect the diff before
  building. Do not run a whole-file automatic formatter on existing source
  files unless explicitly requested — formatter config is advisory guidance
  for new code, not permission to reformat the tree.
- Before committing, run the gate described above and ensure it passes.
- Commit only task-owned changes with plain git commands (`git status`,
  `git add -- <paths>`, `git commit -m "..."`). Use a broad `git add` only
  after verifying every worktree change is task-owned and safe to commit
  (`mingw64/`, `*.exe`, `*.log`, `*.7z` must never be staged — they are
  gitignored, but verify with `git status` anyway).
- Do not push to a remote unless explicitly requested.
- Always consult `llm-wiki/` for non-trivial work in an unfamiliar area; for
  trivial localized work, read only the directly relevant page(s).
- Keep `llm-wiki/` current when durable project knowledge changes. For
  trivial edits with no future-useful context, perform the semantic check
  but skip the wiki edit.
- Mistrust code, code comments, and `llm-wiki` alike — any of them can be
  stale. Verify against current behavior and come to your own conclusion.
- Environment gotchas: `mingw64/` is ~730 MB and regenerable — never commit
  it, delete freely via `python build.py --clean`. `build.py` aborts on
  non-Windows hosts by design. The first build without `mingw64/` downloads
  the toolchain (network-bound, can take minutes); subsequent builds are fast.
- When running the built executable, make sure runs are short, use only safe
  flags, run under a timeout, and that no lingering processes are left behind.

## Engineering rules

- Prefer root-cause fixes over workarounds; do not hide, ignore, weaken, or
  paper over failures.
- Think through the actual root cause of a bug before proposing a fix. If a
  proper fix requires a bigger change, do the bigger change rather than a
  narrow patch that leaves the underlying issue in place.
- Do not use sleeps, wait tables, polling delays, or other timing bandaids
  as crash/race fixes.
- Do not introduce or accept racy, timing-sensitive, or otherwise fragile
  behavior.
- **Safety-predicate discipline (project-specific):** the predicate layer in
  the source (`isExcludedCandidatePath`, `shouldPruneTraversal`,
  `unsafeRecursiveDirectoryTarget`, `isDriverStorePackageRoot`,
  `isCoreDisplayDriverLeaf`, etc.) is what prevents deletion of driver
  packages and core display files. Any change there must preserve the
  fail-closed direction: when in doubt, do NOT match a path for deletion.
  Whole DriverStore package roots stay undeletable; only allow-listed
  subfolders may be removed recursively.
- **Source-file size discipline:** source-code files (Rust modules, build
  scripts included) must stay within roughly **500–800 lines**. Split
  proactively when a file approaches the ceiling — split along the existing
  section boundaries rather than artificially. Wiki/docs pages are exempt.
- Treat logs, dumps, media, captures, credentials, private keys, tokens,
  and user data as sensitive.
- Do not commit secrets, dumps, logs, captures, large generated artifacts,
  or private user data.

## Build, diagnostics, and tests

- Fix newly introduced errors/warnings, plus pre-existing issues in touched
  files or issues that directly block the task; do not expand into
  unrelated repository-wide cleanup.
- There is no automated test infrastructure. Verification is: warning-free
  build + `--list-components` + `--dry-run` inspection of the generated log
  (candidates list and post-run existence check sections). When changing
  matching logic, craft a dry-run against the affected component keys and
  read the candidates section of the log to confirm matches/exclusions
  behave as intended.
- If meaningful logic gets extracted or decoupled from Win32 calls during a
  refactor, consider making it testable, but do not bolt on a test framework
  unprompted.
- Do not add sleeps or timing assumptions anywhere; the code already avoids
  them (event-driven waits, bounded timeouts).

## Debugging and logging

- Add high-signal, rate-limited debug logging when it materially helps
  diagnose state transitions, failures, or regressions; do not add
  unconditional hot-path noise. Route everything through `logLine()` /
  `logAction()` so a run stays ONE log file shared by launcher, elevated
  instance, and SYSTEM worker.
- Ensure builds preserve enough debug information that crash reports and
  logs contain actionable data (the default `-O2` release build has no
  separate debug config; keep `-Wall -Wextra` so misuse surfaces early).

## Debugging tools and paths

| Tool | Purpose | Installed/default path |
| --- | --- | --- |
| `RustyButterKnife.exe --dry-run` | Safe end-to-end scan: exercises discovery/matching and writes the full report without deleting anything | repo root; build first |
| `RustyButterKnife.exe --list-components` | Prints component keys/default states; fast CLI-parse sanity check | repo root |
| `debloat-YYYYMMDD-HHMMSS.log` | One file per run: progress, actions, candidates list, post-run existence check, JSON report block | beside the `.exe`; override with `--log-file PATH` / `--log-dir PATH` |
| `schtasks.exe`, `takeown.exe`, `icacls.exe` | Invoked by the tool itself; always resolved from `%SystemRoot%\System32` by absolute path (never PATH) | `%SystemRoot%\System32` |

## `llm-wiki/` workflow

- `llm-wiki/` is canonical LLM-maintained derived memory, not the sole
  source of truth.
- When starting work on an unfamiliar area — whether to understand or
  change it — first orient via `llm-wiki/repo-map.md` (the code map), then
  read the relevant topic page(s) via `llm-wiki/index.md`, then
  `llm-wiki/log/recent.md` for active/stale-risk areas.
- For substantial work, start with `llm-wiki/index.md`, read only relevant
  topic pages, then read `llm-wiki/log/recent.md`.
- Read archives only when historical context is needed or explicitly
  linked.
- For trivial localized edits, skip broad wiki loading unless the area is
  unfamiliar or stale-risk is likely.
- If `llm-wiki/` is missing during substantial work, create `index.md`,
  `current.md`, `repo-map.md`, and `log/recent.md` by inspecting repo
  structure, build/test entry points, config, docs, and workflows.
- Mistrust wiki claims until verified against code (but mistrust code too),
  tests, build scripts, config, or observed behavior.
- Prefer updating existing pages over creating new ones; create new pages
  only for reusable topics.
- Keep topic pages focused on current best understanding; put chronology,
  partial investigations, and temporary notes in `llm-wiki/log/recent.md`.
- Mark uncertainty explicitly as open question, stale-risk, or unverified
  claim.
- Do not dump raw logs or long command output unless it establishes
  durable knowledge.
- Update the wiki when durable knowledge changes: architecture, behavior,
  build/test/package/deploy/debug workflows, bugs/root causes, invariants,
  conventions, rejected approaches, follow-ups, or code style.
- Do not update the wiki for trivial edits with no future-useful context.
- `llm-wiki/index.md` is a compact routing table with page link, purpose,
  last-verified date, and stale-risk.
- Durable topic pages should include summary, source anchors,
  invariants, diagnostics/failure modes, open questions/stale-risk, and
  last-verified details.
- `llm-wiki/log/recent.md` is newest-first rolling memory; archive older
  entries when it gets too long (see `llm-wiki/log/README.md` for the
  rotation convention).
- After both wiki updates and code changes, perform a semantic quality
  check for contradictions, stale claims, duplicates, orphan pages, broken
  links, missing source anchors, and merge/delete/archive candidates.
