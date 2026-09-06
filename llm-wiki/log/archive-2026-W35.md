# Log Archive (2026-08-26 to 2026-08-27)

Rotated from `log/recent.md` on 2026-09-06 per the rotation convention in
`log/README.md` (recent.md is rotated once it grows past ~200-250 lines).
Entries are newest-first.

## 2026-08-27 — Full-repo audit pass 3: root-reparse traversal fix + abort-aware TI wait

Template audit of the entire repo (Rust primary line-by-line, build/wrapper/
docs/binary). All 2026-08-26 remediation claims verified as genuinely landed.
Findings delivered in chat; recommendable fixes implemented same session.

1. FS-01 ROOT case (High, confirmed by test-first repro): `fsutil::
   for_each_postorder` expanded a junction/symlink ROOT (read_dir follows
   junctions), so `schedule_delete_tree` on a reparse candidate that failed
   direct deletion scheduled the junction TARGET's contents for reboot
   deletion. Existing junction test only covered junction CHILDREN. Fix:
   root is no-follow-probed first — Reparse/File/unreadable roots are
   emitted as single entries, never expanded. Companion fix in
   `deletion::delete_candidate`: PathKind::Reparse candidates now route to
   `remove_tree_counted` (entry-wise removal; DeleteFileW cannot remove a
   directory junction, so the old file-path routing guaranteed failure and
   pushed work into the scheduling fallback). Regression test
   `junction_root_is_emitted_but_never_expanded` fails on the old code.
2. HANG-03/TI-abort: the TI-relaunch parent wait loop was the only
   non-abort-aware wait left (bare 3 s sleep, up to ti_wait_seconds).
   Now checks ABORT_REQUESTED per poll + 300 ms sleep slices;
   `TiRelaunchResult.aborted` plumbed into main → exit code 3 path;
   finish_task() stops+deletes the task on abort (correct: no orphaned
   SYSTEM worker after the user says stop).
3. CLI fail-closed gap: `--log-file --dry-run` consumed the next flag as
   the value, silently dropping a dry-run request from an execute run.
   Flag-like values (leading '-') are now recorded as problems → exit 13
   in execute mode. NOTE: C++ parseArgs retains the old behavior (parity
   abandoned deliberately, fail-closed direction wins).
4. Robustness: `std::env::args_os()` (non-UTF-8 argv no longer aborts
   before the catch_unwind FATAL gate); FATAL finalization is pre-init safe
   (`app::run_opt`, `opts_initialized` guards in write_status_json/log_line).
5. Docs: README --component row now states strict execute-mode rejection.

Gates: cargo fmt/build/clippy -D warnings clean; 53 tests green (51+2);
smokes: --list-components/--version/--help/--dry-run exit 0; execute+badarg
exit 13; execute+flag-like-value exit 13; dry-run+badarg exit 0 warn-only;
README Flags section vs --help diff = 33/33 sync. dist NOT rebuilt (no
release requested; dist artifacts predate these fixes — stale-risk noted).

Falsified/checked this pass — do not re-raise: ERROR_FILE_READ_ONLY=6009 is
a real winerror.h constant; task matching intentionally has no container-
preserve check (C++ parity); discovery walk() root kind is checked; UAC
relaunch vs execute-mutex ordering is correct (relaunch precedes mutex);
zip extraction in build.py is Zip-Slip-safe (stdlib sanitizes '..' parts).

Known leftovers (weighed, see known-debt.md): delete+disable task flag
precedence; 0xFFFF_FFFD abandoned-vs-exit-3 sentinel collision; full-file
historical log reads; dead ti_poll_decision parameter; legacy .cpp header
build-default text stale (file must not grow; README is the contract).

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
