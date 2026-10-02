# Changelog

All notable changes to Sylphra are documented here. The project follows Semantic Versioning.

## [0.0.0-beta1] - 2026-09-23

### Added
- Resource governance layer: a process-wide probe of real working set and system memory availability, a hierarchical reserve/commit/release ledger, and one ceiling table shared by every subsystem.
- Five-level pressure model with hysteresis, including a system-availability driven emergency tier.
- Renderer child processes parse and lay out documents off the user-interface thread and stream the resulting tree over chunked, generation-checked IPC.
- Per-tab live budgets reconciled into the ledger; a tab past its budget spills its document snapshot to disk and restores from it on wake.
- Windows job objects enforce a per-role memory ceiling, children report their own resident memory, and the least important renderer is shed first and brought back through the crash-recovery path.
- Task manager reports measured resident memory, aggregate child memory, per-subsystem ceilings and rejected reservation counts.
- Acceptance, platform and fault-injection coverage for resource governance, plus benchmarks for ledger churn and thousand-tab estimation.
- Document string interning: HTML tag and attribute names share one bounded reference-counted allocation, and the task manager reports the bytes that deduplication saved.
- A `--ram-report=<path>` diagnostic mode that measures real resident memory for an idle core and a twenty-tab session, and times a complete relief sweep.
- A feature-gated renderer memory harness that lets the memory-kill and recovery path be exercised end to end; released builds refuse the command.

### Changed
- The installer script now derives a numeric version for Inno Setup's version fields, so a pre-release identifier such as `0.0.0-beta1` no longer aborts the setup compile.
- Memory relief is triggered by allocation events and a sub-second pulse instead of a fixed minute-long poll, and it reads measured resident memory rather than an estimate.
- A relief pass uses cached per-tab byte totals, so a sweep is linear in tabs instead of re-walking every document tree.
- The job-object memory ceiling now sits above the governance threshold, so the governor retires a runaway renderer before the operating system denies its allocations.
- Subsystem ceilings that exceeded the browser budget (GPU sources, worker process memory, worker responses) were lowered to fit inside it.
- Untrusted input is refused before it can grow a buffer: request bodies by declared length, compressed frames by announced expanded length, document node count and nesting depth, stylesheet source and rule counts, script source size and string allocation, and image pixel dimensions read from the header before decoding.
- Snapshot serialisation is bounded while it is written, so an oversized document never materialises a full copy of itself.
- The masked user agent is derived from the crate version instead of being hard-coded.

### Fixed
- Reconciling a subsystem owner to zero no longer strands the ledger index, so a tab that sleeps and wakes keeps accurate accounting across pressure cycles, and releasing an owner also drops its dead reconciled entries.
- The renderer reply reader bounds each IPC message individually instead of capping the whole stream lifetime, so documents larger than one message budget stream correctly and an oversized line is skipped without tearing down the channel.
- Back and forward navigation on a disk-backed tab clears the spilled snapshot first so a later wake can no longer silently revert the navigation, and an evicted history snapshot now triggers a reload of that URL instead of rendering a blank page.
- Reloading the same URL re-runs the history snapshot budget.

## [0.0.0-alpha] - 2026-09-22

### Added
- Comprehensive project architecture re-engineered into 8 domain modules (`core`, `engine`, `net`, `media`, `storage`, `ui`, `platform`, `experimental`).
- Full integration test suite restructured into 7 domain suites matching `src/` modules (`engine`, `net`, `media`, `storage`, `ui`, `platform`, `acceptance`).
- Native Windows packaging pipeline with Inno Setup installer (`Sylphra-v0.0.0-alpha-Setup.exe`) and portable zip distribution.
- Safe Rust document-focused browser pipeline without embedding Chromium, Gecko or WebKit.
