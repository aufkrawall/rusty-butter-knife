## 2026-09-11 — v2.2.0: AnselCamera made opt-in (driver-breaking deletion)

Root-caused a display-driver failure produced by this tool and turned the
responsible component off by default.

Symptom on the affected machine: an identical D3D11 test binary failed with
`DXGI_ERROR_UNSUPPORTED` (0x887A0004) when named `witcher3.exe` and succeeded
under any other name. The real symptom is not "the app cannot start" but a
per-application feature-level cap: the NVIDIA adapter reported a maximum of
D3D feature level 10_1 for that process, so every `D3D11CreateDevice` asking
for FL >= 11_0 (and every D3D12 device) failed, while a call passing
`pFeatureLevels = NULL` silently got a 10_1 device.

Mechanism (measured on driver r616_69 / 616.92):

- `nvldumdx.dll` (the NVIDIA user-mode D3D loader) references exactly 11 DRS
  setting IDs; two of them gate this path: `0x1035DB89` ("Freestyle Filters
  Allow", global) and `0x1085DA8A` ("Freestyle Filters App Allow", per
  application). It parses `nvdrsdb.bin` itself, so NVAPI/Profile-Inspector
  changes to the *application* profile do not affect the decision; only the
  global flag or the base database does.
- When both are set, two callers of that decision function load
  `<driverstore package>\NvCamera\NvCamera64.dll` (built from
  `GetModuleFileNameW(nvldumdx)` + subdir + filename) and return `E_FAIL`
  (0x80004005) if the load fails. `nvwgf2umx.dll` is then never loaded and
  the adapter stays at FL 10_1.
- Proof: flipping the single value byte of `0x1085DA8A` for the `The Witcher 3`
  profile in `nvdrsdb.bin` (offset 0x13A078 on that driver) from 1 to 0 lifted
  the cap to FL 12_1 with no restart; restoring the byte reinstated it.
- Scope: ~35 shipped NVIDIA game profiles carry `0x1085DA8A = 1`. 31 of 55
  probed profile executable names were affected (The Witcher 3, Conan Exiles,
  Mass Effect Andromeda, Mirror's Edge Catalyst, Watch Dogs 2, Hellblade,
  The Witness, Bulletstorm, Dark and Light, ...).

The tool's `AnselCamera` component (default-on until now) deletes exactly that
payload: run logs show `DeletePath [AnselCamera] ...\NvCamera :: Removed
entries=30` about 80 s after each driver install. The NVIDIA profile flag stays
set after the payload is gone, so the breakage survives driver reinstalls and
looks like a driver bug.

Change: `AnselCamera` is now `default_enabled = false, optional = true` and is
enabled with the new `--include-ansel` flag, wired exactly like
`--include-capture-sdk` (parse, defaults, component selection, elevated-child
argument forwarding). README flag table and `known-debt.md` updated; the
DriverStore payload-deletion debt note now records the measured dependency
instead of calling it a latent risk.

Gate before the bump: `cargo build`, `cargo clippy --all-targets -D warnings`
(clean), `cargo test --all-targets` (64 passed), smokes `--list-components`
(`[ ] AnselCamera (optional)`), `--include-ansel --list-components` (`[x]`),
`--help`.

Not verified here: that restoring `NvCamera64.dll` alone lifts the cap. Only
the negative direction is measured (flag set + payload absent -> E_FAIL ->
FL 10_1). The payload has never been present on the test machine long enough
to check the positive direction.

## 2026-09-06 — Full-repo audit (v2.1.0 tree); CI least privilege, O(n²) collapse fix, wiki drift repair

Template-driven audit of the entire repo (all 25 Rust modules line-by-line,
build.py, CI workflow, README/AGENTS contracts, llm-wiki). Findings delivered
in chat; recommendable fixes implemented same session. Baseline gate green
before changes (build, clippy `-D warnings`, 62 tests, --list-components/
--version/--help smokes); README flag table re-diffed against --help (in sync,
33 flags; `--abort-event`/`--ti-child` internal-only as documented).

Implemented:

1. CI permission scoping: workflow-level `contents: write` dropped to
   `contents: read`; only the release job now requests `contents: write`.
2. Removed provably-unused windows-sys feature `Win32_UI_WindowsAndMessaging`
   (clean `cargo check --all-targets` without it). CORRECTION of the
   2026-08-23 claim that every declared feature is load-bearing: that holds
   for all other features, notably `Win32_System_Registry`, which really does
   gate `ShellExecuteExW`/`SHELLEXECUTEINFOW` in windows-sys 0.60 (verified
   by E0432 without it).
3. `discovery.rs` `collapse_nested_candidates`: canonical keys computed once
   per candidate instead of inside the O(n²) pairwise loop (was O(n²) stat
   calls on candidate-heavy systems). Semantics preserved via
   `canonical_lower_key`/`key_is_parent_of_or_equal`; 2 new unit tests
   (nesting collapse incl. case-variant duplicate; no false prefix collapse
   for `Installer2` vs `Installer2b`).
4. `deletion.rs` `unsafe_recursive_directory_target`: one no-follow stat
   instead of two (identical fail-closed truth table: Missing/File → not a
   directory decision, Directory/Reparse/unreadable → fail-closed input).
5. `tasksched.rs` TI-wait abort path now flushes the child's last log lines
   to the console before the abort WARN (parity with the timeout/finish
   flush paths).
6. `ffi.rs` `current_token_account`: PSID read out of the raw TOKEN_USER byte
   buffer via `read_unaligned` (strictly correct for the u8 allocation).
7. `util.rs`: "CSV line parser" doc comment moved off `now_unique_suffix`
   onto `parse_csv_line`, where it belonged.
8. Wiki drift repair: `codestyle.md` rewritten for the Rust-only tree (C++
   TU references, wrong fmt gate, wrong GPD_VERSION location, stale unsafe-
   family list removed/corrected); `debug-tools.md` .cpp/clang++ references
   replaced; `log/recent.md` rotated into `archive-2026-W34.md`/`-W35.md`
   per the documented ceiling (mojibake em-dash in the oldest entry fixed);
   `log/README.md` rotation table refreshed.

Falsified/checked this pass — do not re-raise: dry-run is genuinely
non-mutating (take_ownership_if_requested is unreachable before the execute
gate in delete_candidate; run_shell_command dry-run path only logs); the
allowlist/EXACT_NAMES process lists are identical and fail closed on drift;
UAC/TI child switch forwarding is complete (enabled map forwarded per
component); `try_acquire_named_mutex` correctly handles WAIT_ABANDONED_0.

Known leftovers re-weighed, left as-is: dead `saw_running_before` param in
`ti_poll_decision`; 0xFFFF_FFFD ABANDONED sentinel unreachable in the UAC
wait (closure never abandons); `--list-components` shows pre-override
defaults (documented debt); abort-event handle opened per abort poll (only
in the UAC-elevated child, ~µs per poll — not material).

## 2026-09-06 — v2.1.0 release prep; dead-code fix; CI green on main

- Version bumped 2.0.1 → 2.1.0 (`Cargo.toml`, `src/app.rs` `RBK_VERSION`,
  `Cargo.lock`). Tag `v2.1.0` is unused; `v2.0.1` was never tagged despite the
  earlier version bump, so 2.1.0 ships the full v2.0.1 hardening plus all
  work since `v2.0.0`.
- Removed dead `TerminateHandle` wrapper from `ffi.rs` (superseded by
  `VerifiedTerminateHandle` in `ffi_process.rs`); it broke the CI clippy
  `-D warnings` gate on the previous head. Gate now green on `main` head
  `29b3a17` (build, clippy, 62 tests, safe smokes).
- README flag table and `--help` verified in sync; exit-code table unchanged.
- Release chronology completed here: after `v2.0.0` (2026-09-05) the tree
  received the v2.0.1 safety hardening (mutation-target verification,
  strict CLI parsing, TI wait hardening — see `known-debt.md` resolved
  findings), the guarded Windows CI/release workflow, COM task enumeration,
  and the named-event UAC cancellation relay.
- First CI-mediated release completed: gate ran on the bumped tree
  (`6f68aa7`), then an empty `[release]` head commit (`2483caf`) with an
  identical tree triggered the release job. Tag `v2.1.0` published on
  2026-09-06 with the PE-verified x86_64 executable; downloaded asset
  reports "Rusty Butter Knife version 2.1.0". Stale merged branch
  `fix/v2.0.1-safety-hardening` deleted.

## 2026-09-05 — Removed legacy PowerShell wrapper & completed audit handoff report

- Removed legacy `Run-RustyButterKnife.ps1` (native executable handles self-elevation directly).
- Removed completed historical document `Rusty Butter Knife — Audit Handoff Summary.md`.
- Updated `README.md`, `AGENTS.md`, `llm-wiki/index.md`, and `llm-wiki/repo-map.md`.

## 2026-09-05 — Project rename to "Rusty Butter Knife", dropped C++, v2.0.0

- Renamed project from GreenPostInstallDebloatNative to "Rusty Butter Knife".
- Crate: `rusty-butter-knife`, binary: `RustyButterKnife.exe`, version `2.0.0`.
- Dropped legacy C++ translation unit (`GreenPostInstallDebloatNative.cpp`) and llvm-mingw dependency.
- Streamlined `build.py` for pure Rust release builds; added automatic privacy-preserving path remapping (`--remap-path-prefix`) to guarantee that local developer paths and usernames are never baked into release binaries.
- Renamed PowerShell wrapper to `Run-RustyButterKnife.ps1` with fallback checks.
- Renamed execution and log mutexes to `Global\RustyButterKnife_Execute_Mutex` and `Global\RustyButterKnife_LogMutex`.

## 2026-09-05 — Full-codebase polyglot hardened audit & quality improvements

Full repository audit (all Rust modules, legacy C++, build system, PowerShell wrapper,
docs, wiki). Implemented 8 concrete improvements:

1. Console LF normalization: unified `encode_crlf_utf16` across `console::out` and
   `console::err_out`, converting bare `\n` to `\r\n` on real consoles without doubling
   existing CRLF. Added unit test.
2. CLI terminal argument parsing: fixed missing value handling when value-taking flags
   (`--log-file`, `--ti-wait-seconds`, `--status-file`, `--log-dir`) appear at the end
   of `std::env::args()`. Correctly classifies as missing value rather than unknown option.
   Added unit test.
3. Path length & attribute clearance: applied `extended_length_path` (`\\?\`) to
   `clear_blocking_attributes` and `set_attrs_normal` so deep paths (>MAX_PATH) do not fail
   read-only attribute stripping; ensured retry branch applies clearance to the extended path.
4. Reparse point deletion routing: `remove_tree_counted` now explicitly inspects
   `FILE_ATTRIBUTE_DIRECTORY` to route directory junctions/symlinks to `remove_dir` and
   file symlinks to `remove_file` with direct error propagation.
5. DriverStore case-insensitivity: updated allowlist folder matching in
   `unsafe_recursive_directory_decision` to use `eq_ignore_ascii_case`.
6. Standardized service/task classification: added `nvvad` and `nvwmi` to `NAME_TERMS`,
   eliminating the one-off check in `service_action_decision` and extending proper
   component classification to scheduled tasks. Added unit tests.
7. Concurrent log cursor tracking: in `attempt_trusted_installer_relaunch`, updated the
   15s heartbeat wait to advance `log_cursor` by the exact written byte length of the
   parent message, preventing race-skipping of child log lines.
8. COM BSTR memory leak resolution: wrapped `var_bstr` output in RAII `VariantGuard`
   calling `VariantClear` on drop in `ffi_tasksched.rs`.
9. Release profile optimization: enabled `lto = "thin"` and `codegen-units = 1` in `Cargo.toml`.
10. PowerShell wrapper fallback lookup: expanded executable search order in
    `Run-GreenPostInstallDebloat.ps1` to check repo root, dist directories, and release targets.

## 2026-09-04 — Root-cause fixes for debloat execution hang / multi-minute lag

Investigation and proper root-cause fixes for debloat cleaning taking ~2+ minutes:

1. Task Scheduler Priority 7 throttled background I/O:
   `register_and_run_ti_child` in `ffi_tasksched.rs` did not configure task priority.
   Task Scheduler defaults to priority 7 (`BELOW_NORMAL_PRIORITY_CLASS` with
   `THREAD_MODE_BACKGROUND_BEGIN` / `IO_PRIORITY_LOW`), causing file deletion and
   scans to be heavily throttled behind Defender real-time scans and OS I/O.
   Explicitly set priority to 4 (`NORMAL_PRIORITY_CLASS`).
2. `schtasks.exe /Query /FO CSV /NH` process execution bottleneck:
   `handle_scheduled_tasks` in `actions.rs` invoked CLI `schtasks.exe` to query all
   system tasks, taking 29+ seconds on standard Windows installations. Implemented
   direct COM `ITaskFolder` recursive enumeration (`TiSession::enumerate_all_task_paths`
   in `ffi_tasksched.rs`), reducing task enumeration time to <0.3s (~100x faster),
   with fallback to `schtasks.exe` (reduced 30s timeout).
3. Broken `%SystemDrive%\Users` path joining in discovery:
   `sys_drive` ("C:") was joined as `PathBuf::from("C:").join("Users")`, producing
   `"C:Users"` (drive-relative path resolved against CWD rather than absolute `"C:\Users"`).
   Fixed to properly format absolute root prefix.
4. Redundant syscalls during directory discovery:
   `discover_candidates` called `path_kind_no_follow` (metadata/symlink probe) on every
   single entry in large search roots before checking if the path matched any component.
   Reordered so `match_component_for_path` filters first; metadata is only probed
   for candidate matches.
5. Mutex abandonment & 30s lock delay:
   `create_global_mutex` created mutexes with initial ownership; `try_acquire_named_mutex`
   closed handles without calling `ReleaseMutex`, abandoning mutexes and blocking subsequent
   acquisitions up to `WAIT_GRACE_MS` (30s). Replaced with RAII `NamedMutexLock` calling
   `ReleaseMutex` on drop and handling `WAIT_ABANDONED_0`. Reduced timeout to 5s.
6. Silent parent wait & live child log streaming:
   Parent process waiting on the background SYSTEM scheduled task previously showed only
   periodic "Still waiting for TI child..." every 15s. Implemented live log streaming
   (`stream_child_log_lines` in `tasksched.rs`) from the shared run log to the console
   at 300ms intervals, giving real-time progress feedback to the user.
