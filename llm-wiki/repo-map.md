# Repo Map (code map)

Last cross-checked: 2026-08-23 (verified against working tree, GPD_VERSION
1.5.0, source ~2500 lines)

Primary sources:
- top-level repo layout (verified against the working tree)
- `build.py`
- `GreenPostInstallDebloatNative.cpp`

## How to navigate this map

`AGENTS.md` points agents at `llm-wiki/index.md` for routing; this page is
the concrete file-level code map. If line anchors here predate a refactor,
treat them as approximate and re-verify (function names are the stable
anchors; the file is a single translation unit).

## Core Tree

- `src/` — **Rust crate (primary implementation)**, ported 1:1 from the
  legacy TU; every module under the ~800-line ceiling:
  `main.rs` (orchestration, wmain flow), `app.rs` (globals/exit codes),
  `types.rs`, `util.rs`, `winfmt.rs`, `console.rs`, `logging.rs`,
  `components.rs`, `options.rs`, `menu.rs`, `sysinfo.rs`, `procs.rs`,
  `matching.rs`, `discovery.rs`, `actions.rs`, `deletion.rs`,
  `report.rs`, `tasksched.rs`; plus the FFI boundary family
  (`ffi.rs`, `ffi_services.rs`, `ffi_tasksched.rs`) — the ONLY modules
  containing `unsafe` (crate root denies it elsewhere). Build via
  `cargo build --release`; gate adds clippy -D warnings + fmt.
- `GreenPostInstallDebloatNative.cpp` — LEGACY C++17 single TU, kept as
  reference and still buildable; must not grow. Section map by first
  defining line (the Rust modules in `src/` mirror these sections 1:1):
  - 1–77: header comment block (usage, safety model, build examples) — keep
    in sync with behavior changes; then includes and `GPD_VERSION`.
  - ~79–175: exit-code enum (`EXIT_*`), `kTiTaskPrefix` ("NvDebloatTI-"),
    structs (`Options`, `Component`, `Candidate`, `ActionRecord`,
    `RunState`) and globals (`g_options`, `g_state`, `g_componentEnabled`,
    `g_abortRequested`).
  - ~179–345: string/util helpers: `toLower`, `wildcardMatchNoCase`,
    `quoteArg`, `widen`, `decodeProcessOutput` (OEM codepage), JSON helpers
    (`jsonEscape`, `jsonFindStringField`, `jsonFindIntField`).
  - ~419–460: console color + logging core: `consoleColor`, `logLine`,
    `logAction`, `addAction`. ALL output goes through here so one run = one
    log file shared by launcher/elevated/SYSTEM processes.
  - ~466–550: process execution: `ProcessResult`, `runProcessCapture`,
    `runShellCommand`; privilege checks: `isAdmin`, `systemDirFile`
    (System32 absolute paths only), `currentTokenAccount`,
    `isTrustedInstaller`.
  - ~621–690: `buildComponents()` — the bloat-component catalog (keys,
    display names, default-enabled flags, leaf globs). Optional components
    (NGX, HDAudio, PhysX, NotebookOptimus, VirtualAudio, NvWMI,
    CaptureSDK) are default-off. UpdateAndProfileUpdater is kept off in
    wizard defaults on purpose.
  - ~693–950: CLI surface: `initializeComponentSelection`, `printUsage`,
    `parseArgs`, `parseBoolAssignment`, `applyComponentArgs`,
    `interactiveMenu`. Flag table in README must match this section.
  - ~951–1027: `getExePath`, `initializeRunState` (single-instance mutex),
    `writeStatusJson` (child→parent status handoff).
  - ~1028–1330: TrustedInstaller relaunch machinery: `sweepStaleTiArtifacts`
    (cleans orphaned NvDebloatTI-* tasks/status files),
    `effectiveChildSwitches`, `attemptTrustedInstallerRelaunch` (scheduled-
    task COM API, task name `NvDebloatTI-<pid>`, 2 h execution limit;
    falls back to SYSTEM/admin per flags).
  - ~1332–1665: matching/predicate layer — THE SAFETY CORE:
    `isNvidiaContextString`, `isPreservedContainerName`,
    `isBloatProcessName`, DriverStore guards (`leafHasInfArchMarker`,
    `isDriverStorePackageRoot(Name)`, `isDriverStoreSidecarIni`),
    `isDeveloperToolPath`, `isCoreDisplayDriverLeaf`,
    `isExcludedCandidatePath`, `shouldPruneTraversal`,
    `matchComponentForPath`, `unsafeRecursiveDirectoryTarget`,
    `collapseNestedCandidates`, `discoverCandidates`. Fail-closed direction
    is mandatory here (see AGENTS.md).
  - ~1667–1770: `tallyPreviousLogs` (history scan of old run logs),
    `writeCandidatesCsv` (despite the name: appends candidates to the run
    log, no separate file), `inspectNvContainerModules`.
  - ~1772–1975: actions on live system: `killLockerProcesses`,
    `serviceMatchesBloat`, `handleServices`, `taskMatchesBloat`,
    `handleScheduledTasks` (OEM-codepage task-name decoding).
  - ~1976–2117: deletion layer: `takeOwnershipIfRequested` (takeown/icacls
    via well-known SIDs), `extendedLengthPath` (`\\?\` fallback),
    `clearBlockingAttributes`, `scheduleDeleteOne/Tree` (MoveFileEx reboot
    deletion), `deleteCandidate`.
  - ~2118–2331: reporting/orchestration: `writeReport` (JSON report block
    into the log), `verifyCandidateRemoval` (post-run existence check),
    `runCleanup`.
  - ~2332–2499: `relaunchElevatedForWizard` (UAC relaunch with forwarded
    selection), `consoleCtrlHandler` (graceful abort → exit code 3),
    `wmain`.
- `build.py` — LEGACY toolchain bootstrap + compile driver. Pins llvm-mingw
  (`LLVM_MINGW_VERSION`), downloads/SHA256-verifies/extracts into `mingw64/`
  if missing, falls back to system `clang++`. Compile flags: `-std=c++17
  -municode -O2 -Wall -Wextra -static`; targets x86_64 (default) and aarch64
  via `--target=aarch64-w64-mingw32`. Both arches write the same output name.
- `Run-GreenPostInstallDebloat.ps1` — user-facing wrapper: self-elevates via
  UAC, runs the full destructive flag set, keeps window open, forwards extra
  args; honors `GPD_NO_PAUSE=1` for automation.
- `README.md` — user-facing docs: what-it-does, build, usage, full flag
  table, exit codes, safety notes, log format. Treated as a contract that
  must be updated alongside CLI/behavior changes.
- `.gitignore` — excludes `mingw64/`, `*.exe`, `*.log`, `*.7z`, `_extract/`,
  `/target` (Rust build dir). `Cargo.lock` is committed (binary crate).

## Important Support and Output Paths

- `mingw64/` — bundled llvm-mingw toolchain (~730 MB). Regenerable at any
  time via `python build.py`; removable via `python build.py --clean`. Never
  commit.
- `GreenPostInstallDebloatNative.exe` — build output (gitignored). Both
  arch targets write this same filename (see known-debt.md).
- `debloat-YYYYMMDD-HHMMSS.log` — run logs beside the exe (gitignored).
- `llvm-mingw-<version>-ucrt-x86_64.zip` / `_extract/` — download cache /
  temp extraction dir used by `build.py`; cleaned up automatically.
- `GreenPostInstallDebloatNative.7z` — manual release snapshot (gitignored).

## High-Risk / High-Value Files

- `GreenPostInstallDebloatNative.cpp` — runs elevated as TrustedInstaller/
  SYSTEM and deletes real system files. The predicate layer (~lines
  1332–1665) is the safety boundary protecting driver packages and core
  display files; careless changes there have maximal blast radius.
- `build.py` — downloads and EXECUTES a compiler toolchain; hash pinning
  via `--sha256` exists for supply-chain reasons. Keep the pinned version +
  URL scheme intact unless deliberately changing policy.

## Practical Notes

- Single-file architecture is deliberate (see AGENTS.md); do not split.
- The header comment block of the .cpp duplicates key usage/safety claims —
  update it together with README when behavior changes (three-way sync:
  header comment ↔ printUsage ↔ README flag table).
- Exit codes are an API surface for the wrapper script and parent→child TI
  handoff (0/1/2/3/10/11/12); changing them breaks both.
- `writeCandidatesCsv()` writes INTO the log, not a CSV file — the function
  name is historical; don't "fix" it without reading it.
