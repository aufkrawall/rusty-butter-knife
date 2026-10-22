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

## Generic debugger / binary-analysis tools (agent side)

Baseline from `aufkrawall/llm-prompt-templates` (default integration,
2026-10-22). These are tools an agent may use to inspect artifacts; they are
unrelated to how the product itself resolves `System32` tools.

- Verify a tool exists and runs before relying on it; treat documented paths
  as hints. Never mutate binaries, symbols, global debugger flags, registry or
  system settings unless explicitly requested.
- Treat dumps, logs (including `debloat-*.log`), captures and extracted
  strings as potentially sensitive.
- Resolution order, first reliable source wins:
  1. `debug-tool-manifest.json` written by `tools/discover-debug-tools.ps1`
     (non-mutating; default output `%LOCALAPPDATA%\LLMDebugTools\`, run e.g.
     `pwsh -File tools/discover-debug-tools.ps1 -ProjectRoot .`);
  2. local, uncommitted `tool-paths.env` (template: root
     `tool-paths.example.env`);
  3. repository-local/pinned tools (none today besides the Rust toolchain);
  4. `Get-Command` / `where.exe`;
  5. documented known-good paths; 6. safe defaults.
- Typical Windows tools when installed: `cdb`/`windbg`/`WinDbgX`,
  `dumpchk`, `symchk`, `dbh`, `dumpbin` (`link /dump`), `undname`,
  `llvm-objdump`, `llvm-strings`/`strings`, Sysinternals `procdump`,
  `procmon`, `procexp`, `handle`, `listdlls`, `vmmap`, `sigcheck`.
- Crash dumps: combine `srv*` with the local symbol directory
  (`cdb -z <dump> -y "srv*;<symbol dir>" -c ".ecxr; k; q"`); Rust PDBs are
  emitted under `target/`.
- Security-oriented inventory (PE hardening, strings/secrets, signing,
  runtime mitigation, tracing, evidence capture) and this project's
  privileged-boundary anchors: `llm-wiki/debug-tools-security-audit.md`.
- Windows-only project: the template's Linux/macOS tool tables are not
  carried here (see the security-audit page for cross-platform coverage).
- Never generate or commit `debug-tool-manifest.json`, `tool-paths.env` or
  other machine-specific paths (gitignored).
