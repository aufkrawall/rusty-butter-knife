## 2026-08-26 — Audit Handoff Summary remediation (9-step patch order)

All P0/P1 findings from `GreenPostInstallDebloatNative — Audit Handoff
Summary.md` implemented on the Rust primary; commits b72505b..abf699a (+doc
commits). Claims verified against source FIRST — several wiki claims were
wrong, see corrections below.

1. HANG-01 (b72505b): capture_process moved to new `ffi_capture.rs`.
   PeekNamedPipe-gated drain ONLY (no ReadFile-until-EOF after child exit),
   terminate+1 s grace timeout path w/ typed `timed_out`, abort-aware waits,
   SetHandleInformation fail-closed, STARTUPINFOEXW +
   PROC_THREAD_ATTRIBUTE_HANDLE_LIST so children inherit ONLY the pipe.
   Tests: hung-child, high-volume stdout, descendant-holding-stdout (the
   old blocking read is caught by an elapsed-time assertion).
   Gotchas hit: windows-sys 0.60 HANDLE=*mut c_void (not isize);
   InitializeProcThreadAttributeList(NULL) probe ALWAYS fails with
   ERROR_INSUFFICIENT_BUFFER while filling size (do not treat as fatal);
   lpValue must outlive list deletion → ProcThreadAttrs owns both.
2. SAFETY-01 (d5c358b): `matching::ActionDecision` classification layer;
   kill_locker_processes/handle_services/handle_scheduled_tasks now take the
   enabled map; deselected components SKIP-log all their actions; unknown
   targets fail closed. Also batched process termination with ONE shared
   3 s convergence deadline.
3. SERVICE-01/02 + SEC-01 (d5c358b): desired_access_for_ops(stop,cc,delete)
   split with exact-mask tests (wiki's earlier 'already fixed' claim was
   FALSE — see known-debt); QueryServiceStatusEx convergence w/ typed
   StopOutcome; system_dir_file returns Result, callers skip stages rather
   than fall back to PATH/CWD bare names.
4. FS-01/REPORT-01/02 (9c3f34b): new `fsutil.rs` PathKind no-follow probes +
   abort-aware iterative postorder walker (children-before-parents,
   junction entries never descended); schedule_delete_tree rewired to it;
   success-only reboot_scheduled_paths set drives honest pending-reboot
   reporting; tri-state existence replaces exists(); unsafe_recursive target
   fails closed on unreadable stat. Run IDs gained PID+counter suffix.
5. CLI/pause/TI-exit (2f5157d): strict BoolAssign parser, malformed values
   collected; execute mode REJECTS before mutation -> exit code 13;
   mixed-case --COMPONENT= fixed (was silently dropped); ti-wait clamped
   15..=7200 & rejects non-numeric; --no-ti-relaunch w/o fallback exits 10
   not 1; pause is uniform finalization (fixes elevated-window closing) w/
   opts_initialized() guard for --help paths.
6. Waits/cancellation (b34d795): takeown locale-letter memoized process-wide
   (stop N*120 s amplification); UAC parent wait sliced+abort-aware
   (wait_exit_code_aborting).
7. Logging (fa75092): append_utf8_file fallible + one-shot stderr report;
   run_cleanup fails closed when log unwritable (verified live: FATAL on
   System32 probe); named-mutex serialized large sections; status-write
   failures surfaced.
8. Build (9e9084d): explicit triples BOTH legs + PE machine verification;
   DEFAULT variant rust (C++ reference-only); README build section synced.
9. Docs/tests (this commit): tasksched ti_poll_decision core + 3 tests;
   README safety notes; AGENTS gate includes cargo test; wiki pages current.

Gates re-run per patch: cargo fmt, clippy -all-targets -D warnings clean,
51 tests green, safe smokes (--help/--version/--list-components/--dry-run)
executed live; exit codes 13/10 verified live. README flag table diffed vs
--help (33/33 sync).

CORRECTIONS of 2026-08-23 entries (they overclaimed):
- "least-privilege services" had NOT landed then; only on d5c358b.
- TI fast-completion diagnosis: 2026-08-23 only handled the past-grace case;
  immediate LastTaskResult consult (fast exit between polls before 60 s)
  landed here as abf699a.

# Recent Activity

## 2026-08-23 — Second pass: unit tests for the pure safety layer + hardening

- New `#[cfg(test)]` suites (std test attribute only, no framework), 24
  tests total, all passing; `cargo clippy --all-targets -- -D warnings`
  added to the personal gate for this pass:
  - `util.rs`: wildcard matcher (star/question/empty/case), quote_arg
    Windows argv rules incl. backslash runs, join_command, parse_csv_line
    quoted/escaped forms, atoi_prefix clamping, json_escape/json field
    readers, case-insensitive helpers.
  - `matching.rs`: inf arch markers, package-root/sidecar detection,
    core-display-leaf protection, nv-token context rule (2-letter yes,
    3-letter no), preserved containers, telemetry module globs, and
    match_component_for_path scenarios (Installer2 guard, USBTypeC payload
    inside a package, NGX on/off exclusion, developer-tool pruning).
  - `deletion.rs`: `unsafe_recursive_directory_target` refactored into a
    pure decision fn (`unsafe_recursive_directory_decision`) — behavior
    identical (is_dir/full/parent/leaf inputs); tests cover package roots,
    allow-listed vs non-allow-listed dirs, files never flagged.
  - `options.rs`: parse_bool_assignment variants.
- Hardening implemented:
  - `sysinfo.rs`: is_trusted_installer now exact-matches
    NT SERVICE\TrustedInstaller or its well-known service SID
    (S-1-5-80-956008885-…, confirmed via MSDN/sc showsid) instead of a
    substring match. Fail direction unchanged-safe.
  - `ffi_services.rs`: ScmGuard::open_service_full replaced by
    open_service_for_ops(stop_needed, reconfigure_needed) — least-privilege
    handle rights so DACL-denied DELETE can't block stop-only flows;
    call site in actions.rs updated.
  - `Run-GreenPostInstallDebloat.ps1`: Format-NativeArgument pre-quotes
    passthrough args containing spaces/quotes (Start-Process -ArgumentList
    does NOT re-quote); verified with char-code-level PS tests.
  - `build.py`: LLVM_MINGW_SHA256 pin (from GitHub release asset digest)
    verified automatically on fresh downloads; --sha256 overrides.
- Test-authoring lessons: the bash transport halves literal backslash runs —
  generate backslash-heavy test text programmatically (chr tokens), never
  via heredoc literals. CRT quote rule re-confirmed: only backslashes
  IMMEDIATELY before a quote double (k → 2k+1); earlier ones flush singly.
- Dist artifacts rebuilt for both legs; stale root exe removed.

## 2026-08-23 — Full-repo quality audit; safety/robustness fixes applied

- Template-driven audit of the entire repo (Rust primary, legacy C++,
  build.py, PS wrapper, README, dist binaries). Findings delivered in chat;
  recommendable fixes implemented directly. Gates re-run: cargo build +
  clippy -D warnings + fmt clean, `python build.py --variant cpp`
  warning-free, `--list-components`/`--help`/`--dry-run` smoke-tested,
  README flag table diffed against --help (33/33 in sync).
- Deliberate Rust-only divergences from the C++ reference (accepted, they
  improve safety/robustness):
  - Execute-mode single-instance mutex failure is now FATAL + exit 12
    instead of WARN-and-continue (fail-closed for destructive runs).
  - TI-relaunch wait: a child that ran and exited between polls was
    misdiagnosed after the 60 s start-grace as "task never started"; the
    parent now consults LastTaskResult first and only reports "never
    started" for SCHED_S_TASK_HAS_NOT_RUN (0x41303).
  - UAC wizard relaunch forwards --log-file via quote_arg/join_command
    instead of manual quoting.
- app.rs global-state locks recover from poisoning so the FATAL handler can
  still write report/status artifacts after an unexpected panic.
- Help text --status-file reworded truthfully in BOTH implementations (the
  flag is still honored when passed; only internal use is gone). README
  synced: build section describes both legs/dist layout/--variant/cargo
  requirement; exit-code 12 row covers mutex-unavailable; new safety note
  discloses component-matched single-file deletions inside DriverStore
  packages (verified live: nv_dispi/nvhda/nvmedisk/nvraid/nvdimm packages).
- Falsified during audit — do NOT re-attempt pruning "unused" windows-sys
  features: windows-sys 0.60 cfg-gates ReadFile behind Win32_System_IO and
  ShellExecuteExW/SHELLEXECUTEINFOW behind Win32_System_Registry; every
  declared feature is load-bearing.
- Binary hardening verified on dist artifacts via llvm-readobj/llvm-strings:
  ASLR + high-entropy VA + NX + stack cookies (+CFG check infra in Rust),
  system-DLL-only imports, no embedded secrets/URLs/user paths. Rust CFG
  function table empty (GuardCFFunctionCount=0) — informational only.
- Accepted audit leftovers recorded in known-debt.md. Untracked foreign
  page llm-wiki/debug-tools-security-audit.md (Green Curve content from
  another project) flagged for removal/move — not deleted without owner
  confirmation.

## 2026-08-23 - Unified build script: python build.py now builds both variants

- Default `python build.py` produces `dist/cpp-<arch>/` and
  `dist/rust-<arch>/` binaries; `--variant {all,cpp,rust}` picks a leg.
- Each folder keeps its own run logs (logs are written beside the exe).
- `--clean` extended to remove `dist/`; `--arch` applies to both legs
  (Rust aarch64 requires the matching rustup target).
- C++ source retained untouched as reference per user directive.



## 2026-08-23 — Full Rust port (v1.5.0) landed alongside legacy C++



- New Cargo crate at repo root: `src/` split into 17 modules, all under the

  ~800-line ceiling (largest: `ffi.rs` at ~740).

- Unsafe confined to the `ffi*` module family (`ffi.rs`, `ffi_services.rs`,

  `ffi_tasksched.rs`) behind safe wrappers; crate root has

  `#![deny(unsafe_code)]`. Verified: zero unsafe blocks elsewhere.

- `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt` all clean;

  release binary ~660 KB fully static.

- Golden parity on this machine vs same-day C++ run: identical discovery

  summary (all components = 0), byte-compatible log markers; Rust log

  tallying correctly counts earlier logs (cross-version contract holds).

- Contracts preserved: exit codes, status-file JSON keys, single-run single-

  log append across processes, flag surface incl. bare-launch wizard and

  EXECUTE confirmation, TI relaunch via COM (S4U/TrustedInstaller principal,

  PT2H limit, LastTaskResult polling).

- Known text divergence: --help says "native Rust build" instead of

  "native C++ build".



## 2026-08-23 â€” Project set up as git repo; AGENTS.md / llm-wiki filled in



- Initialized local git repo (`main`, initial commit `797cd95`) with

  `.gitignore` covering `mingw64/`, `*.exe`, `*.log`, `*.7z`.

- Copied in generic `AGENTS.md` / `llm-wiki` templates and adapted them to

  this project: documented the `python build.py` gate, safe-flag-only

  verification policy (`--dry-run`/`--list-components`), the source section

  map of the single translation unit at GPD_VERSION 1.5.0, code style

  (C++17 wide-char, 4-space/LF/K&R), and accepted debt (no test suite,

  single TU, arch output-name collision).

- Verified claims against source: exit codes enum, component catalog

  (`buildComponents()`), TI relaunch machinery, `writeCandidatesCsv`

  writing into the run log only.

- Rust port feasibility assessed (see `rust-port-feasibility.md`): 100%

  portable via `windows(-sys)` + gnullvm targets; no hard blockers; managed

  risks are log-marker/status-JSON/exit-code contracts and dry-run

  golden-master verification given the absent test suite.

