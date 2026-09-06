# Log Archive (2026-08-23)

Rotated from `log/recent.md` on 2026-09-06 per the rotation convention in
`log/README.md` (recent.md is rotated once it grows past ~200-250 lines).
Entries are newest-first.

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

## 2026-08-23 — Project set up as git repo; AGENTS.md / llm-wiki filled in

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
