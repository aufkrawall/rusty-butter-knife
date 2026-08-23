# Recent Activity

## 2026-08-23 - Unified build script: python build.py now builds both variants

- Default `python build.py` produces `dist/cpp-<arch>/` and
  `dist/rust-<arch>/` binaries; `--variant {all,cpp,rust}` picks a leg.
- Each folder keeps its own run logs (logs are written beside the exe).
- `--clean` extended to remove `dist/`; `--arch` applies to both legs
  (Rust aarch64 requires the matching rustup target).
- C++ source retained untouched as reference per user directive.



## 2026-08-23 — Full Rust port (v1.5.0) landed alongside legacy C++



- New Cargo crate at repo root: `src/` split into 17 modules, all under the

  ~800-line ceiling (largest: `ffi.rs` at ~740).

- Unsafe confined to the `ffi*` module family (`ffi.rs`, `ffi_services.rs`,

  `ffi_tasksched.rs`) behind safe wrappers; crate root has

  `#![deny(unsafe_code)]`. Verified: zero unsafe blocks elsewhere.

- `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt` all clean;

  release binary ~660 KB fully static.

- Golden parity on this machine vs same-day C++ run: identical discovery

  summary (all components = 0), byte-compatible log markers; Rust log

  tallying correctly counts earlier logs (cross-version contract holds).

- Contracts preserved: exit codes, status-file JSON keys, single-run single-

  log append across processes, flag surface incl. bare-launch wizard and

  EXECUTE confirmation, TI relaunch via COM (S4U/TrustedInstaller principal,

  PT2H limit, LastTaskResult polling).

- Known text divergence: --help says "native Rust build" instead of

  "native C++ build".



## 2026-08-23 â€” Project set up as git repo; AGENTS.md / llm-wiki filled in



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

