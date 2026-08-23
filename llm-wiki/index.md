# llm-wiki Index

Last cross-checked: 2026-08-23 (initial fill-in from template; verified
against working tree at GPD_VERSION 1.5.0)

Primary sources:
- `AGENTS.md`
- `GreenPostInstallDebloatNative.cpp` (the entire product: single ~2500-line
  C++17 translation unit)
- `build.py` (toolchain bootstrap + compile driver)
- `Run-GreenPostInstallDebloat.ps1` (elevation wrapper)
- `README.md` (user-facing flags/exit codes/safety docs — must stay in sync
  with the CLI)

## Purpose
`llm-wiki` is the derived documentation layer for agents and maintainers.
It collects repo knowledge that is useful to consult quickly, but it is not
the concrete implementation.

## Trust Model
- Always consult `llm-wiki` before non-trivial work.
- Do not blindly trust it, especially after a period of active change in a
  given area.
- Cross-check important claims against code, build scripts, config files,
  and current behavior.
- Update the relevant page and `log/recent.md` whenever you confirm drift,
  fill a gap, or change project behavior.
- Do not conflate the wiki with the project code. If the wiki and the code
  disagree, the code/build script wins.

## Recommended Read Order
- Start here to find the right page.
- If you are about to understand or change code in an unfamiliar area,
  first orient with `repo-map.md` (the code map: layout, section ownership
  inside the single TU, build pipeline, safety predicates) before reading
  the topic page.
- Read `current.md` next for a compact current-state summary and routing.
- Read `log/recent.md` after that when you need recent historical context
  for a changing area. For older entries, consult the relevant
  `log/archive-*.md` file.
- For build/tooling questions, see the build pipeline section of
  `repo-map.md`, plus `codestyle.md`.

## Content Catalog
- `codestyle.md`
  - C++17 wide-char conventions, naming, formatting, logging discipline;
    Python style in `build.py`. Last verified 2026-08-23.
- `current.md`
  - Compact current-state summary and token-efficient routing into the
    longer wiki pages.
- `repo-map.md`
  - **Code map**: top-level layout, section-by-section map of the single
    translation unit, build pipeline structure, and important paths.
- `known-debt.md`
  - Deliberately accepted debt with its reasoning, so later audits don't
    re-derive it and later agents don't "fix" it without weighing the same
    trade-off. Includes the arch-output-collision and no-test-suite items.
- `debug-tools.md`
  - Safe verification commands, run-log anatomy, and diagnostic workflows.
- `log.md`
  - Stub pointing to `log/recent.md` (recent activity) and
    `log/archive-*.md` (older archives).

## Page Maintenance
- Each page should keep `Last cross-checked`/`Last verified`, `Primary
  sources`, and an `Open questions / stale-risk` section current.
- Plan docs and old comments may be useful context, but they are secondary
  sources. Prefer live code, build scripts, and config.
- Keep `current.md` compact and current. Move detailed history into the
  topical pages and `log/recent.md` (or the relevant archive) instead of
  expanding the compact entrypoint indefinitely.
