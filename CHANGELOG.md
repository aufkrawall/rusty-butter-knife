# Changelog

## Unreleased

## v2.3.0 - 2026-10-22

### New

- **Lock holder diagnostics:** when a file cannot be deleted, the log names the process that holds it (read-only Restart Manager lookup; nothing is stopped because of it).
- **Earlier-run awareness:** a run reports NVIDIA deletions from an earlier run that are still waiting for a restart.

### Improved

- **Readable console output:** the console shows a short narrative with section headings, `WARNING:` markers and a closing Result section that says what happened and whether a restart is needed; technical detail stays in the log file.
- **Stage summaries:** services, processes and scheduled tasks each report what was checked and why NVIDIA entries were or were not changed, including the kept NVIDIA Display Container.
- **Expected locks are not warnings:** files kept in use only by the NVIDIA Display Container (loaded telemetry plugins, its open telemetry log) are reported as "will be deleted at the next restart" instead of as failures.

### Fixed

- **One locked file blocked a whole folder:** a single in-use file (for example in `Installer2` right after a driver install) no longer keeps every other file of that folder; only the in-use entries are deferred to the next restart.
- **False restart warning:** runs that queued nothing for the next restart, including already-clean systems, no longer end with a restart warning.
- **Kept modules announced as pending deletions:** loaded modules of deselected components (for example the profile updater) are no longer reported as needing a restart to be deleted.
- **Merged log lines:** the post-run summary line no longer runs into the next log line.
- **SYSTEM worker warning:** the privileged worker running as SYSTEM (the normal case) is no longer reported as a warning.
