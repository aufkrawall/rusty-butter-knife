## 2026-10-22 — Agent tasks end with a fresh dist binary

User request: after an agent finishes, `dist/` must always hold a new binary.
`AGENTS.md` now makes `python build.py --arch x86_64` the last step of every
task (after gate + commit), so `dist/x86_64/RustyButterKnife.exe` matches
`HEAD`; a failed build must be reported, not hidden.

## 2026-10-22 — Log-driven fixes: best-effort deletion, lock diagnostics, honest reporting

Reviewed four real v2.2.0 execute-run logs (2026-09-23 … 2026-10-22, drivers
`da865…` → `feedb…` → `20f49…`). No unsafe behavior; reboot-queued deletions
did complete (the post-reboot run found 0 candidates). Fixed what the logs
showed:

- `remove_tree_counted` aborted on the first error, so one in-use file in
  `Installer2` (2026-10-06, right after a driver install) kept every sibling
  and queued 165 entries for reboot. Now `remove_tree_best_effort` continues
  per entry; only leftovers are queued. Covered by a held-handle test and a
  junction no-follow test.
- Leftovers are explained via Restart Manager (new `ffi_filelocks.rs`,
  read-only, budgeted) and container-loaded modules (`lockinfo.rs`). Expected
  container locks (loaded `_nvtopps.dll` etc., newer drivers also load
  `NvTelemetry64.dll`; `GameSessionTelemetry\telemetryPlugin.log`) are INFO;
  anything else stays WARN with holder names.
- Closing "Some locked-file deletions may require reboot" fired
  unconditionally (also on a 0-candidate run); replaced by a `Reboot required`
  notice only when MoveFileExW queued something.
- Post-run `Summary:` lacked its newline (next log line was glued on).
- Loaded modules of deselected components (`nvprofileupdaterplugin.dll` with
  UpdateAndProfileUpdater off) were announced as reboot deletions; module
  messages now follow the component selection and actual scheduling.
- Every run logged a WARN that the worker was SYSTEM, not TrustedInstaller.
  That is the designed outcome: now INFO when the token is LocalSystem
  (SID-checked), WARN otherwise. Requesting SYSTEM explicitly is deferred
  (known-debt) because it needs execute-mode VM verification.
- Added per-stage summary lines (services/tasks/locker processes) and a
  read-only `PendingFileRenameOperations(2)` check.

Not changed on purpose: no retries/sleeps on sharing violations, no killing
of NVIDIA containers, no rename of loaded DLLs, no exit-code/status-file
contract change. Verified: build, clippy `-D warnings`, 80 tests, `--version`,
`--help`, non-admin `--dry-run`. Execute mode not exercised (VM-only policy).

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
