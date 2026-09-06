# Debug Tools

Last cross-checked: 2026-09-06 (Rust-only tree)

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

Sections in order: progress/log lines → `==== Candidates ==== ` table →
per-action records → post-run existence check (paths remaining, with reason:
dry-run / pending reboot deletion / failure) → final JSON report block.
`tally_previous_logs()` additionally embeds a history summary from prior logs
in the same directory.

## Tool path resolution

- The tool resolves its own dependencies: system executables strictly from
  `%SystemRoot%\System32`. `build.py` resolves `cargo` from PATH or
  `~/.cargo/bin` and always passes the explicit MSVC target triple.
- No other external tool paths are involved; do not introduce PATH-based
  resolution of system executables in elevated contexts.
