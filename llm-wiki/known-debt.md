# Known and Accepted Debt

Last verified: 2026-08-23

Primary sources:
- `AGENTS.md`
- `build.py` (ARCH_TARGETS / OUTPUT handling)
- `GreenPostInstallDebloatNative.cpp`

## Purpose

Debt that has been deliberately accepted, with the reasoning, so later
audits do not re-derive it and later agents do not "fix" it without weighing
the same trade-off. Items here are *recorded*, not endorsed. Anything that
becomes cheap or safe to fix should be fixed and removed from this page.

## No automated test suite

The project has zero tests. Verification is manual: warning-free build,
`--list-components`, and `--dry-run` log inspection.

Why accepted: nearly all logic is coupled to live Win32 state (services,
scheduled tasks, the real filesystem under NVIDIA install paths), so unit
testing would require an abstraction layer disproportionate to a single-
author utility, and integration tests would run destructive operations on
real systems. What would need to be true to resolve: the predicate/matching
layer gets decoupled from direct Win32 calls, or a fixture-tree-based dry-run
harness is added.

## Single ~2500-line translation unit (LEGACY C++ ONLY)

All C++ lives in one file; compile time is still fine but growing, and edits
touch one giant diff surface.

Why accepted: deliberate architecture choice of the legacy implementation —
trivial distribution/build story (`python build.py`, one source file).
The file is grandfathered as historical reference and must NOT grow.
Per user directive, source files target ~500–800 lines: the Rust port
(`src/`) is split into modules under that ceiling, and any new source file
must respect it. Do not split the legacy .cpp.

## Both architectures write the same output filename

`build.py --arch aarch64` overwrites `GreenPostInstallDebloatNative.exe`
(the x86_64 binary). `build.py` prints a `[!]` reminder but does not prevent
it.

Why accepted: keeps paths simple for a single-artifact workflow. To resolve:
per-arch output names or an output flag would be needed; not worth it while
ARM64 builds are occasional.

## Function name `writeCandidatesCsv` is historical

It appends the candidates table INTO the unified run log; no separate CSV
file is produced (README correctly says none are created).

Why accepted: renaming churns the file for no behavioral gain; the comment
inside explains it. If touched anyway, a rename to e.g.
`appendCandidatesToLog` is welcome.

## Falsified findings — do not re-raise

- "README claims no CSV files but code has writeCandidatesCsv()" — checked
  2026-08-23, confirmed not a defect: the function writes into the run log
  only (see debt entry above).
- "Cargo.toml declares unused windows-sys features" — checked 2026-08-23,
  falsified as a cleanup: windows-sys 0.60 cfg-gates `ReadFile` behind
  `Win32_System_IO` and `ShellExecuteExW`/`SHELLEXECUTEINFOW` behind
  `Win32_System_Registry`; every declared feature is load-bearing.

## Audit leftovers (2026-08-23, weighed and accepted)

Deferred findings from the full-repo audit, each deliberately accepted.
Items resolved in the same-day second pass are listed at the bottom:

- Scheduled-task matching picks the first CSV field starting with `\` as the
  task path because schtasks `/FO CSV` column order varies between versions;
  a UNC-style hostname in column 1 could theoretically confuse it. Not
  observed on any tested system.
- `--list-components` prints default component states before
  `--component=` args are applied (wmain ordering parity with the legacy
  C++). Treated as documented behavior.
- Opt-in components (NGX/HDAudio/CaptureSDK/NvWMI) intentionally match
  payload files inside DriverStore packages; deleting them corrupts those
  packages while leaving active driver copies untouched. Now disclosed in
  README safety notes; behavior kept because these components' targets live
  exclusively inside packages. Default-on AnselCamera vs historical
  `NvCamera*.dll` inside old nv_dispi packages remains a version-dependent
  latent risk of the same class.
- COM VARIANT helpers in `ffi_tasksched.rs` intentionally leak BSTRs on
  success paths (process-lifetime objects, few calls per run) — mirrors
  `_variant_t` lifetime simplification, commented in code.
- `console::err_out` does not convert LF→CRLF on real consoles (cosmetic;
  stderr is usually redirected).

Resolved same day (second audit pass), recorded here so they are not
re-derived:

- `is_trusted_installer()` no longer substring-matches; it accepts exactly
  `NT SERVICE\TrustedInstaller` or its well-known service SID
  (S-1-5-80-956008885-…). Fail direction stays safe (false negative only
  triggers an extra relaunch attempt / WARN line).
- The llvm-mingw archive SHA256 is now pinned in `build.py`
  (`LLVM_MINGW_SHA256`, taken from the GitHub release asset digest) and
  verified automatically on every fresh download; `--sha256` overrides.
- Service handles are opened with rights matched to the requested
  operations, so DACL-denied DELETE no longer blocks stop/disable flows.
