# Rust Port Feasibility Assessment

Last verified: 2026-08-23 (against GPD_VERSION 1.5.0)

Primary sources:
- `GreenPostInstallDebloatNative.cpp` (full API-surface grep)
- `build.py`
- https://doc.rust-lang.org/rustc/platform-support/windows-gnullvm.html
- https://github.com/rust-lang/compiler-team/issues/877

## Verdict

**Technically yes — 100% portable with no hard blockers.** Every Win32/COM
API used is covered by Microsoft's official `windows`/`windows-sys` crates,
and the self-contained llvm-mingw build story can be preserved via the Rust
`*-pc-windows-gnullvm` targets (Tier 2 with std + host tools; llvm-mingw is
the explicitly recommended C toolchain for them). But "no negative side
effects" requires actively managing a short list of parity risks — see
below. A port removes latent memory-bug classes but does NOT make the
debloating logic itself safer: the danger lives in predicate logic, which
Rust expresses identically.

## Dependency inventory → Rust coverage

| C++ dependency | Used for | Rust coverage |
| --- | --- | --- |
| CreateProcessW/CreatePipe/ReadFile/WriteFile/WaitForSingleObject | runProcessCapture, external tools (schtasks/takeown/icacls) | `windows-sys` 1:1 |
| CreateToolhelp32Snapshot + Process32*/Module32*W | killLockerProcesses, inspectNvContainerModules | `windows-sys` 1:1 |
| OpenSCManager/EnumServicesStatusEx/OpenService/ChangeServiceConfig/DeleteService | handleServices | `windows-sys` or `windows-service` crate |
| OpenProcessToken/GetTokenInformation/CheckTokenMembership | isAdmin/isTrustedInstaller/currentTokenAccount | `windows-sys` 1:1 |
| MoveFileExW (FILE_DELAY_UNTIL_REBOOT), SetFileAttributesW, DeleteFileW | reboot-time deletion, attribute clearing | `windows-sys` 1:1 |
| CreateMutexW | single-instance execute guard | `windows-sys` 1:1 |
| SetConsoleCtrlHandler/SetConsoleTextAttribute/GetStdHandle | graceful abort (atomics only — no unwinding across callback), colors | `windows-sys` 1:1 |
| ShellExecuteExW("runas") | UAC wizard relaunch | `windows-sys` 1:1 |
| GetOEMCP/MultiByteToWideChar/WideCharToMultiByte | OEM-codepage decode of schtasks CSV output | `windows-sys` 1:1 |
| Task Scheduler COM: CoInitializeEx, CoCreateInstance(ITaskService), GetFolder/NewTask, IRegistrationInfo, IPrincipal(TrustedInstaller, TASK_LOGON_S4U), ITaskSettings(PT2H), IExecAction, RegisterTaskDefinition(TASK_CREATE_OR_UPDATE), IRegisteredTask::Run, ITaskFolder::GetTasks/DeleteTask | TI relaunch machinery (~300 lines) | `windows` crate (BSTR/VARIANT RAII replaces _bstr_t/_variant_t); proven feasible, fiddliest part of the port |
| std::filesystem directory_iterator with skip_permission_denied + error_code | custom pruned traversal (no recursive_directory_iterator used) | `std::fs::read_dir` + same manual recursion design |
| Registry APIs | none used (0 hits) | n/a |

No MFC/ATL beyond `_bstr_t`/`_variant_t`. No CRT-specific tricks. No third-
party libraries at all today.

## Build-story preservation

- Current: pinned llvm-mingw downloaded by build.py; `-static` fully static
  exe; x86_64 default, aarch64 cross via `--target=aarch64-w64-mingw32`.
- Rust equivalent: `x86_64-pc-windows-gnullvm` / `aarch64-pc-windows-gnullvm`
  targets (Tier 2 w/ host tools since 2025), using the SAME bundled
  llvm-mingw as linker/C-toolchain; `+crt-static` keeps binaries fully
  static. build.py's download/verify/extract flow stays useful as-is.
- Supply chain: today zero deps. Keep the posture by depending ONLY on
  `windows`/`windows-sys` (+ lockfile commit, optionally `cargo vendor`).
  Everything else in this codebase is hand-rolled already (JSON escape/
  parse, glob matching, quoting) and ports mechanically.

## Negative side effects to manage (the honest list)

1. **Log-format coupling**: `tallyPreviousLogs()` parses old
   `debloat-*.log` files for the exact markers `run header ====` and
   `Candidate count: `. A Rust rewrite must keep these byte-compatible or
   history tallying silently degrades on pre-port logs.
2. **Cross-process contracts are API surface**: exit codes (0/1/2/3/10/11/12)
   consumed by Run-GreenPostInstallDebloat.ps1; child→parent status-file
   JSON parsed by jsonFindStringField/jsonFindIntField; single-run single-log
   file appended by launcher/elevated/SYSTEM processes. All must be preserved
   exactly.
3. **Exception→panic mapping**: wmain's try/catch maps to exit codes 1/2;
   Rust needs a catch_unwind boundary + panic hook to keep that contract.
4. **Encoding/quoting helpers**: quoteArg/joinCommand (Windows argv quoting),
   wildcardMatchNoCase, OEM-codepage decode are subtle; behavioral drift here
   changes matching results on localized systems. Needs golden-output diffs.
5. **Verification gap**: no test suite exists (known-debt.md), so the safety
   net during rewrite is `--dry-run` output comparison between the C++ and
   Rust builds on the same machine (candidates list should match line-for-
   line). This is the main reason a port is riskier than routine work here.
6. **AV/SmartScreen reputation resets**: a new binary from a different
   toolchain loses whatever file reputation the old hash had; expect fresh
   heuristic friction for an unsigned privileged utility. Language-agnostic
   but operationally real.
7. **Binary size**: static Rust exe likely ~2–4 MB vs 1.8 MB now. Cosmetic.
8. **COM verbosity**: Task Scheduler COM in Rust is more explicit/unsafe-ish
   than C++ with comdef RAII; bounded scope but where new bug classes would
   be introduced if rushed.

## What does NOT improve from porting

- Safety of deletions: governed by fail-closed predicates, identical logic
  in any language.
- Locale robustness, DriverStore guards, TI relaunch semantics: re-derived,
  not language-given.
- The tool remains destructive-by-design; dry-run discipline unchanged.

## Recommended path if ever executed

1. Port in dependency order: utils/predicates → discovery → actions → COM
   last; keep C++ build green throughout.
2. Golden-master harness: scripted C++ vs Rust `--dry-run` diff (log minus
   timestamps) on the dev machine before first release build.
3. Byte-compatible log markers + status JSON keys; freeze exit codes.
4. Single dependency (`windows-sys`), vendored, lockfile committed.

## Falsified concerns — do not re-raise

- "Task Scheduler COM not available in Rust" — false: generated bindings
  exist in the `windows` crate (Win32/System/TaskScheduler feature).
- "Need MSVC for a Windows Rust build" — false for gnullvm targets; llvm-mingw suffices (Tier 2 w/ host tools).

## Open questions / stale-risk

- Exact `windows` crate feature set / ergonomics may have shifted after
  2026-08; re-check before starting any port.
- Tier status of gnullvm targets verified 2026-08-23; unlikely to regress.
