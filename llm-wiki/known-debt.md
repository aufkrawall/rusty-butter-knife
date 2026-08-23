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

## Single ~2500-line translation unit

All C++ lives in one file; compile time is still fine but growing, and edits
touch one giant diff surface.

Why accepted: deliberate architecture choice — trivial distribution/build
story (`python build.py`, one source file) matches the project's philosophy.
Do not split unless explicitly requested.

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
