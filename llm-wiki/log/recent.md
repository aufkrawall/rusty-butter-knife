# Recent Activity

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
