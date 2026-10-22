# Debug Tools

Last cross-checked: 2026-10-22 (log signals for locks/reboot queue)

Primary sources:
- `AGENTS.md`
- `README.md` (Logs & reports section)
- `src/logging.rs` (log core), `src/main.rs` (`write_status_json`),
  `src/app.rs` (exit-code constants)

## Tools

| Tool | Purpose | Installed/default path |
| --- | --- | --- |
| `python build.py` | Rebuild after any source change; warnings are the primary static analysis | repo root |
| `RustyButterKnife.exe --list-components` | CLI-parse + component-catalog sanity check; instant | repo root (build first) |
| `RustyButterKnife.exe --dry-run` | Safe end-to-end exercise of scan/match/report without deleting anything | repo root (build first) |
| `debloat-YYYYMMDD-HHMMSS.log` | THE diagnostic artifact: one file per run containing progress, actions, candidates list, post-run existence check, JSON report | beside the `.exe`; relocate via `--log-file PATH` / `--log-dir PATH` |
| `schtasks.exe` / `takeown.exe` / `icacls.exe` | Invoked by the tool itself for task/ownership ops | always `%SystemRoot%\System32` (absolute-path resolution is a security invariant) |

## Diagnostic Workflows

### Verifying a matching/exclusion change

1. `python build.py` or `cargo build` — must be warning-free.
2. `./RustyButterKnife.exe --dry-run --log-file "$env:TEMP\rbk-test.log"`
   (optionally plus the relevant `--include-*` flag).
3. Read the `==== Candidates (...) ====` section: confirm new paths match /
   protected paths don't. Check DriverStore package roots are never listed
   as whole-package candidates.
4. Check the post-run existence-check and JSON report sections at the end.

### Diagnosing a failed TrustedInstaller relaunch

1. Exit code 10 = TI relaunch failed/not permitted; 11 = TI child reported
   failure. The single run log contains both parent and child output.
2. Look for orphaned `NvDebloatTI-*` scheduled tasks / `-status.json`
   files; these are swept automatically at relaunch time
   (`sweep_stale_ti_artifacts`) but can be inspected manually via
   `schtasks /Query | Select-String NvDebloat`.
3. A second execute-mode instance refuses to start with exit code 12
   (single-instance mutex).

### Reading a run log

Line format: `[YYYY-MM-DD HH:MM:SS] [LEVEL] message`, always single-line.
Levels: `INFO`/`WARN`/`ERROR`/`FATAL`/`DRYRUN`/`SKIP`, `STEP` (section
heading) and `DETAIL` (file-only: never printed, never streamed). The console
renders records as `HH:MM:SS  message` (`WARNING:`/`ERROR:` prefixes,
`== heading ==` for STEP) via `logging::render_console`; the TI parent streams
worker lines through the same renderer and skips untimestamped block lines.

Sections in order: header (`[DETAIL]` run-header marker, RunId, identity,
components; then the human `[STEP]` header) → "Services, processes and
scheduled tasks" → "Looking for NVIDIA bloat" → `==== Candidates ====` table
→ "Removing" / "What would be removed" → post-run existence check block →
JSON report block → "Result" section → `Full log:` line.
Cross-version markers parsed by `tally_previous_logs()` stay verbatim
(`run header ====`, `Candidate count: N`), now on DETAIL lines.

Signals worth knowing (2026-10-22):
- `Services:` / `Processes:` / `Scheduled tasks:` lines prove each stage ran
  and explain every NVIDIA entry (kept container, component off, not
  recognized, to change).
- `Waiting for a restart:` reads `PendingFileRenameOperations(2)` (raw counts
  on a DETAIL line); NVIDIA entries mean an earlier run still waits for a
  restart. `N of the M item(s) found are already queued` ties them to items.
- Loaded container modules: "can only be deleted after a restart" (targeted)
  vs "kept (its component is off)"; full paths are DETAIL lines.
- `In use - will be deleted at the next restart [...]` (INFO) is the expected
  loaded-DLL/open-log case. `Could not delete` / `Could not fully delete`
  (WARN) lists each `- not removed:` entry with its Restart Manager holders —
  the first place to look for an unexpected locker (installer, AV scan, ...).
  Individual `Queued for deletion at restart` entries are DETAIL lines.
- The Result section asks for a restart only when MoveFileExW queued
  something in this run, and counts the run's WARN/ERROR lines.
- The 2–25 s gap before the launcher's `The elevated window finished` line
  in wizard runs is the elevated console's `--pause` prompt, not a hang.

## Tool path resolution

- The tool resolves its own dependencies: system executables strictly from
  `%SystemRoot%\System32`. `build.py` resolves `cargo` from PATH or
  `~/.cargo/bin` and always passes the explicit MSVC target triple.
- No other external tool paths are involved; do not introduce PATH-based
  resolution of system executables in elevated contexts.
