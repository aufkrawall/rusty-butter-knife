# GreenPostInstallDebloatNative — Audit Handoff Summary

## Scope and validation status

The repository was reviewed statically across:

- primary Rust implementation under `src/`;
- legacy `GreenPostInstallDebloatNative.cpp`;
- `build.py`;
- `Run-GreenPostInstallDebloat.ps1`;
- Cargo/build configuration;
- README, AGENTS, and `llm-wiki`;
- existing unit tests.

The available audit environment does not contain `cargo`/`rustc` and is not Windows, so the Win32 paths could not be dynamically exercised. `build.py` passes Python syntax compilation. No destructive execution was performed.

The downloadable report contains the complete findings, evidence, source locations, implementation guidance, regression tests, recommended commit sequence, and definition of done.

## Release-blocking findings

### P0 — HANG-01: External-process timeout can block indefinitely

**Primary source:** `src/ffi.rs:498-638`  
**Legacy source:** `GreenPostInstallDebloatNative.cpp:466-533`

`capture_process()` correctly uses `PeekNamedPipe` while the direct child is running, but after the child exits—or after `TerminateProcess` is called because of timeout—it performs an unconditional blocking `ReadFile` loop at `src/ffi.rs:614-629`.

Because the child is launched with inherited handles, a descendant can retain the stdout/stderr pipe's write handle. The direct child can therefore be gone while the pipe still has a writer. The final `ReadFile` waits for data or EOF and can block indefinitely.

This defeats the advertised:

- 60-second `schtasks` mutation timeout;
- 120-second `schtasks /Query` timeout;
- 120-second `takeown`/`icacls` timeout.

**Required repair:**

1. Never perform an unbounded pipe read after process completion.
2. Extract a `drain_available()` operation based on `PeekNamedPipe`.
3. On normal exit, drain only currently available bytes and close the pipe without waiting for EOF.
4. On timeout:
   - set a typed `timed_out` result;
   - terminate the direct child;
   - wait for termination using only a short bounded grace;
   - drain available bytes;
   - close the pipe regardless of descendants.
5. Check `SetHandleInformation` errors.
6. Prefer `STARTUPINFOEX` with `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` so only intended handles are inherited.
7. Add Windows regression helpers for:
   - a descendant retaining stdout after its parent exits;
   - a genuinely hung child;
   - large-volume stdout.

This should be the **first patch**.

---

### P0 — SAFETY-01: Component deselection does not protect all associated system objects

**Sources:** `src/actions.rs:117-223`, `src/actions.rs:349-476`, `src/main.rs:120-128`, `src/report.rs:369-376`

File discovery honors the enabled-component map, but process, service, and scheduled-task handling do not consistently do so.

Examples:

- `kill_locker_processes()` has no component-state input.
- `service_matches_bloat()` checks component state only for VirtualAudio; broad terms such as `telemetry`, `update`, `profileupdater`, `frameview`, `shadowplay`, etc. bypass it.
- `task_matches_bloat()` does not receive the component map at all.

This directly conflicts with the wizard behavior that deselects `UpdateAndProfileUpdater` and tells the user that the update/profile-updater stack will be kept.

**Required repair:**

Create one classification layer:

- `classify_process(...) -> Option<ComponentKey>`
- `classify_service(...) -> Option<ComponentKey>`
- `classify_task(...) -> Option<ComponentKey>`

Every mutation must require:

`target has known component && component is enabled && global operation flag permits mutation`

Unknown/unclassified targets must fail closed.

Regression tests must prove that turning a component off prevents **file, process, service, and scheduled-task** modifications for that component.

---

## Other high-priority defects

### SERVICE-01 — Service “least privilege” access is not actually least privilege

`src/ffi_services.rs:130-151` combines `SERVICE_CHANGE_CONFIG` and `DELETE` whenever either reconfiguration operation is requested.

Consequences:

- disable-only unnecessarily requires DELETE permission;
- delete-only unnecessarily requires CHANGE_CONFIG permission.

A service DACL can therefore reject an operation that should otherwise be permitted.

Split `stop_needed`, `disable_needed`, and `delete_needed`, and unit-test the exact access masks.

The wiki currently claims this issue was already fixed; that documentation is wrong.

### SERVICE-02 — Service shutdown is requested but never awaited

`ControlService(...STOP...)` is followed immediately by further cleanup. A service can still be exiting and holding the files the next stage attempts to remove.

Implement bounded `QueryServiceStatusEx` convergence and distinguish stopped, already stopped, timeout, and query failure.

### HANG-02 — Fast SYSTEM/TI worker completion can cause an unnecessary ~60-second wait

`src/tasksched.rs:206-280` polls every three seconds but does not inspect `LastTaskResult` for a missed fast completion until the 60-second start grace has elapsed.

A worker that starts and finishes between two polls can therefore produce approximately one minute of apparently pointless waiting.

Check `LastTaskResult` immediately whenever the task is no longer Running/Queued. Keep waiting only while it remains `SCHED_S_TASK_HAS_NOT_RUN`.

### FS-01 — Windows reparse points need an explicit destructive boundary

Discovery/deletion uses a mixture of no-follow and follow-style filesystem calls such as `p.is_dir()`. No code explicitly rejects `FILE_ATTRIBUTE_REPARSE_POINT` before recursive destructive operations.

Implement one Windows-specific no-follow path-kind helper. Never recurse through junctions/reparse points. A matching reparse entry may be removed itself, but its target must never be traversed.

### SEC-01 — System32 lookup fails open

`src/sysinfo.rs:24-27` warns that executing a bare system-tool name in the privileged worker would be dangerous, then does exactly that if `GetSystemDirectoryW` fails.

Change this to `Result<PathBuf, ...>` and fail closed. `schtasks.exe`, `takeown.exe`, and `icacls.exe` must never be launched through PATH/CWD resolution in the privileged worker.

The PowerShell wrapper should likewise avoid bare `powershell.exe` for its elevation relaunch.

### REPORT-01 — Failed reboot-delete scheduling is reported as successful

`ScheduleDelete` records can have `INFO` or `WARN`, but `verify_candidate_removal()` converts **every** such record to `SCHEDULED_REBOOT`.

A failed `MoveFileExW` can therefore be reported as “scheduled for deletion at next reboot.”

Only successful scheduling may produce that state. Prefer typed action/outcome enums rather than status strings.

### REPORT-02 — `Path::exists()` can generate false deletion success

`Path::exists()` returns false for metadata errors as well as genuine nonexistence.

Final verification can therefore classify a permission-denied/inaccessible path as successfully gone.

Use `try_exists()` or `symlink_metadata()` and distinguish:

- NotFound;
- exists;
- verification error.

---

## Secondary cleanup-delay amplifiers

The following are bounded after HANG-01 is repaired, but can still make cleanup substantially slower.

### Serial process waits

`src/actions.rs:139-189` waits as long as three seconds **for each** terminated process. N slow processes can cost approximately `3 × N` seconds.

Terminate all selected processes first, then use one shared ~3-second convergence deadline.

### Ownership retries

`src/deletion.rs:14-65` tries `takeown` with four locale-specific answers. Each invocation has a 120-second timeout, followed by another 120 seconds for `icacls`.

Nominal maximum: approximately **600 seconds per candidate**.

Prefer native SID/security APIs. If external utilities remain, enforce one candidate-level budget rather than five independent full timeouts.

### Reboot-delete traversal

`schedule_delete_tree()` recursively collects every descendant, materializes them all in a vector, sorts them, and performs/logs a `MoveFileExW` call for each.

Large residual trees can make this stage expensive. Convert it to abort-aware iterative/postorder traversal without storing and sorting the complete tree.

### Historical log scanning

`tally_previous_logs()` fully reads every historical `debloat-*.log` into memory.

Stream the files instead and consider limiting historical scope because this is reporting work, not cleanup-critical work.

### Ctrl+C is not propagated into waits

The console handler sets `ABORT_REQUESTED`, but subprocess capture and the UAC parent's infinite child wait do not inspect it.

Make both waits cancellation-aware.

---

## CLI and orchestration defects

The CLI should be made substantially stricter because this is destructive software.

Current behavior includes:

- unknown arguments are warned about but ignored;
- invalid component values become `on` unless they resemble an explicit false value;
- invalid `--preserve-nvcontainers=` values similarly become true;
- mixed-case `--Component=` handling is inconsistent;
- malformed component assignments can silently disappear;
- `--ti-wait-seconds` can reach roughly 68 years;
- `--no-ti-relaunch` without fallback can ultimately return generic fatal exit code 1 instead of documented TI-specific exit code 10.

Execute mode should reject malformed or unknown inputs **before any mutation**.

There is also a pause-flow bug. The elevated wizard child is deliberately given `--pause`, but successful/failing TI orchestration returns `Flow::Early`, and `Flow::Early` bypasses `maybe_pause_on_exit()`. This contradicts the README statement that the elevated window remains open.

Pause behavior should be explicit finalization state, not a side effect of whether an internal branch used `Early`.

---

## Logging and auditability

`append_utf8_file()` silently drops open/write errors. `write_status_json()` similarly ignores errors.

For a destructive system utility, failure to establish an audit log should be detected before destructive operations. Prefer fail-closed execute-mode behavior if the selected run log cannot be written.

The parent and worker also append concurrently to one file. Since the final JSON report is one large multi-line write, explicit cross-process serialization should be added to prevent possible interleaving.

Run IDs have only one-second resolution, making unrelated same-second runs capable of sharing a default log filename. Add PID/random/millisecond uniqueness to the root run ID and continue forwarding the same run identity to children.

---

## Build/distribution defects

`build.py` contains a real architecture bug:

`RUST_ARCH_TARGETS["x86_64"] = None`

means “use host target,” not “build x86_64.”

On an ARM64 Windows host, asking for `--arch x86_64` can therefore produce an ARM64 executable under `dist/rust-x86_64`.

Every architecture must map to an explicit target triple. After compilation, verify the PE machine type before copying the artifact.

The repository also has a tooling-policy contradiction:

- live Rust configuration uses `*-pc-windows-msvc`;
- project wiki describes `*-pc-windows-gnullvm`;
- AGENTS says no MSVC support is implied;
- README makes the Rust build sound as though cargo alone is sufficient.

Choose either a real MSVC build strategy with documented prerequisites or a real gnullvm/llvm-mingw strategy and align all configuration/documentation.

---

## Legacy C++ recommendation

`build.py` currently defaults to building **both** Rust and C++.

The legacy C++ version contains several important divergences:

- the same process-capture pipe hang;
- mutex creation failure merely warns and destructive execution continues;
- every matched service is opened with STOP + QUERY + CHANGE_CONFIG + DELETE;
- the fast scheduled-task completion case is handled worse than Rust.

The preferred approach is to change the default build/distribution to **Rust only** and clearly designate C++ as reference code.

If C++ continues to be published as an executable, all P0/P1 safety fixes must be mirrored there and parity tested.

---

## Documentation/test-state problems

Repository guidance is stale enough to affect implementation decisions:

- source contains **24 Rust unit tests**, despite AGENTS/wiki saying there are zero tests;
- the wiki says architecture builds overwrite one another, but current `build.py` uses distinct `dist/<variant>-<arch>` directories;
- the wiki says service access-right separation was fixed, but current source proves otherwise;
- current gnullvm/MSVC guidance conflicts;
- README's updater-preservation claim is not true for services/tasks/processes;
- README's elevated-window pause claim is not true on all orchestration paths.

Update these documents only after the implementation patches are landed so they accurately describe tested behavior.

---

# Required patch order

1. **Fix subprocess capture hard bounds.**
2. **Gate every action type by component state.**
3. **Correct service rights and wait for service shutdown.**
4. **Harden reparse/existence/reboot-delete semantics.**
5. **Make system-tool and CLI preconditions fail closed.**
6. **Reduce serial/repeated cleanup waits and propagate cancellation.**
7. **Fix logging/audit reliability.**
8. **Fix architecture builds and decide Rust-vs-C++ distribution policy.**
9. **Add Windows integration tests and correct repository documentation.**

# Minimum regression gate

The next implementation session should add or run:

- `cargo test --all-targets`;
- `cargo clippy --all-targets -- -D warnings`;
- inherited stdout-pipe descendant test;
- hung-child timeout test;
- high-volume subprocess-output test;
- component-off classifier/action tests;
- exact service-right mask tests;
- scheduled-task fast-completion state-machine tests;
- failed reboot-delete report test;
- junction/symlink no-follow tests;
- malformed destructive CLI tests;
- output PE-architecture verification.

Application-level smoke tests must remain non-destructive:

- `--help`;
- `--version`;
- `--list-components`;
- `--dry-run`;
- component-selection dry runs.

Do not use a normal workstation's real NVIDIA installation as the initial execute-mode regression environment.

# Definition of done

The remediation is not complete until:

- subprocess deadlines remain bounded even when descendants inherit stdout;
- turning a component off prevents all file/process/service/task actions for it;
- service operations request only needed permissions;
- service shutdown completion or timeout is explicitly observed;
- recursive operations cannot cross reparse points;
- permission/I/O verification errors cannot masquerade as deletion success;
- failed `MoveFileExW` calls cannot be reported as pending reboot;
- privileged utilities are never launched using bare names;
- malformed/unknown execute-mode arguments fail before changes;
- abort signals can escape subprocess waits promptly;
- x86_64/ARM64 output architecture is explicit and verified;
- the legacy implementation is either safety-equivalent or no longer distributed by default;
- destructive execution cannot silently proceed without a usable audit trail;
- repository documentation reflects the actual tests and implementation.