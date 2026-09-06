# Known and Accepted Debt

Last verified: 2026-09-06

Primary sources:
- `AGENTS.md`
- `.github/workflows/windows-ci-release.yml`
- `build.py`
- current Rust source under `src/`

## Purpose

Debt that has been deliberately accepted, with the reasoning, so later
audits do not re-derive it and later agents do not "fix" it without weighing
the same trade-off. Items here are *recorded*, not endorsed. Anything that
becomes cheap or safe to fix should be fixed and removed from this page.

## No sanitizers/fuzzing; partial destructive-path coverage

As of v2.0.1 the Rust crate has 61 tests (`cargo test --all-targets`) and a
Windows GitHub Actions gate that runs fmt check, build, clippy `-D warnings`,
all tests and safe CLI smokes on pushes/pull requests. Tests include real
Windows integration coverage for bounded subprocess capture and junction
no-follow behavior plus pure-decision cores for service masks, TI wait state,
CLI parsing, reporting and mutation-target safety.

Still missing: sanitizers/fuzzing and execute-mode regression on real
destructive targets. By policy, execute-mode regression must run only on a
sacrificial VM. Matching/report logic is well covered, but truly destructive
Win32 call sites are still exercised only indirectly or via dry-run. A future
improvement would be a disposable VM harness with synthetic NVIDIA-like
fixtures and snapshot rollback.

## Scheduled-task CSV fallback heuristic

The fast path enumerates Task Scheduler through COM. If COM enumeration fails,
the `schtasks /Query /FO CSV` fallback picks the first CSV field starting with
`\` because column order varies between Windows versions. A UNC-style hostname
in an unexpected field could theoretically confuse that fallback. Not observed
on tested systems, and the COM path normally avoids it entirely.

## `--list-components` shows defaults before component overrides

`--list-components` exits before `--component=` arguments are applied. This is
kept as documented historical behavior. Changing it would be straightforward
but would alter an existing CLI contract for little practical benefit.

## DriverStore package-interior payload deletion

Opt-in components (NGX/HDAudio/CaptureSDK/NvWMI) intentionally match payload
files inside DriverStore packages. Deleting those files corrupts the package
copy while leaving active installed copies untouched. README discloses this.
Whole package roots remain protected and recursive DriverStore deletion is
allow-listed. Default-on AnselCamera versus historical `NvCamera*.dll` inside
old nv_dispi packages remains a version-dependent latent risk of the same
class.

## External ownership utilities

`takeown.exe`/`icacls.exe` are still used for ownership changes. The locale
Yes-letter winner is memoized process-wide, but native security APIs would
remove the external-process timeout surface entirely. Deferred because adding
AdjustTokenPrivileges/SeTakeOwnership code expands the unsafe boundary for
marginal benefit under the normal SYSTEM/TI execution context.

## Resolved findings — do not re-raise without new evidence

- v2.0.1: process termination no longer trusts generic basename substring
  matches. Only explicit known process names reach the mutation path, and the
  live full image is verified through the same handle used for termination.
- v2.0.1: service/task mutation requires a strong NVIDIA anchor in addition to
  semantic matching, blocking arbitrary `Nv...` + generic `update/share/...`
  combinations.
- v2.0.1: the unelevated launcher no longer abandons its wait and returns exit
  3 while the elevated destructive child continues. Ctrl+C is relayed across
  the UAC boundary and the launcher remains attached until child exit.
- v2.0.1: `--ti-wait-seconds` uses full-string integer parsing; values such as
  `abc1` or `600junk` are rejected instead of prefix-parsed.
- v2.0.1: `--status-file` is preserved through UAC/TI success paths.
- v2.0.1: `build.py` always passes the explicit requested Rust target, so an
  x86_64 build on an ARM64 Windows host actually cross-compiles rather than
  relying on PE verification to reject a wrong-host artifact.
- `is_trusted_installer()` exactly matches `NT SERVICE\TrustedInstaller` or its
  well-known service SID; substring identity matching is gone.
- Service handles use least-privilege operation-specific access masks with
  exact-mask tests.
- COM VARIANT BSTR ownership is RAII-managed by `VariantGuard` +
  `VariantClear`.
- Console output normalizes bare LF to CRLF without doubling existing CRLF.
- Subprocess capture is bounded and does not block on descendant-inherited
  stdout handles.
- Recursive filesystem operations do not traverse reparse points, including a
  reparse point used as the traversal root.
- Execute mode rejects malformed/unknown CLI arguments before any mutation.
- Build artifacts are PE-machine verified and compiler paths are remapped.
