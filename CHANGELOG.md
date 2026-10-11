# Changelog

All notable changes to Sylphra are documented here. The project follows Semantic Versioning.

## [0.0.0-beta2] - Unreleased

### Added
- Governance groundwork for an on-demand Chromium compatibility backend: a dedicated `WebView` resource subsystem with its own ceiling, a `WebViewHost` process role bounded by a per-process memory limit and Windows job-object headroom, and a composite budget of 700 MB soft / 900 MB hard that applies only while a compatibility tab is live.
- A `compat-webview2` cargo feature, gated to Windows builds and off by default, that keeps the native-first path lean; release packaging and CI build with it enabled so the shipped browser carries the compatibility backend.
- A Windows-only WebView2 host module (behind `compat-webview2`) that lazily creates one shared Chromium environment and attaches per-tab controller surfaces through a non-blocking create driven by a UI subscription, with navigation, bounds, and visibility control. The browser executable links under `--locked`.
- A compatibility rendering path: the WebView2 host is located by the browser window handle and placed within the content area below the toolbar (DPI-scaled inset), so the tab strip and omnibox stay visible. Ctrl+Shift+G toggles the compatibility surface for the active tab, and a native http(s) load failure now falls back to it automatically. Live surfaces hide or show with the active tab and re-anchor to the content area when the window is resized. Background suspension and controller recycling stay bounded by the shared policy. The browser imports `WebView2Loader.dll` at runtime, which release packaging installs beside the executable.
- A compatibility governance module that measures the resident memory of the WebView2 host process tree and plans background suspension and controller recycling against the shared ceiling table, with unit coverage for subtree walking, parent cycles, and shed arithmetic. The browser reconciles the measured WebView2 process-tree working set into the `WebView` ledger subsystem, and on the memory-pressure tick it suspends hidden background surfaces and recycles them when the measured tree exceeds the `WebView` ceiling, resuming the foreground surface.
- Release packaging now places `WebView2Loader.dll` beside the browser executable in the portable ZIP, the installer, and CI artifacts, which the Windows (GNU) WebView2 backend requires at runtime.
- Compatibility surfaces honor privacy: incognito tabs open in WebView2 InPrivate mode, and the environment stores browsing data under the browser's own profile directory instead of a shared default, keeping partitioned storage and incognito isolation for the compatibility backend. Subresource requests inside the WebView2 surface are routed through the browser's request blocker via the WebResourceRequested event, answering matching ad and tracking requests with an empty blocked response.

### Changed
- The idle and native-tab budgets are unchanged: an idle core still targets 90 MB and twenty native tabs still stay within 500 MB. The widened composite limits take effect only when a compatibility backend tab is foregrounded.
- The Rust worker stack floor was raised from 32 MB (16 MB on CI) to 64 MB, because code generation for the WebView2 COM bindings overflowed the smaller stacks during full binary builds.
- `compat-webview2` now owns its dependencies: `webview2-com` and the aliased `windows` 0.58 bindings are optional, so a native build neither compiles nor links them.
- The fast and release test tiers, and the CI quality job, now type-check and run unit tests with `compat-webview2` enabled; previously the whole compatibility host was only ever compiled by clippy and the release build.
- Packaging treats a missing `WebView2Loader.dll` as an error: `packaging/install.ps1` installs it, `package.ps1` refuses to stage without it, CI fails the portable staging, and the Inno Setup entry no longer skips a missing source. The loader is now selected by the build's own architecture instead of the first match in the build output, which could otherwise bundle an arm64 or x86 loader into an x64 package.

### Fixed
- A compatibility surface request can no longer wait forever. Every request carries a deadline, an environment that fails or times out is recorded as permanently unavailable, and the tab returns to the native error page instead of staying blank behind a UI timer that never retires.
- The host window is now matched only among windows owned by this process, so a hung third-party window cannot stall the interface during handle discovery and a window this build does not own cannot receive the rendered surface.
- Every path that abandons a created controller now closes it, and retargeting a tab whose create is still in flight reuses that create instead of starting a second controller.
- WebView2 memory measurement deduplicates the shared browser process, takes one process snapshot per sample, is rate-limited to one sample per second, and the `WebView` ledger keeps reconciling while surfaces stay live rather than only while they are being created.
- The declared host caps are now enforced: concurrent controllers are bounded with least-recently-used recycling, only one surface stays foreground, hidden surfaces suspend under pressure, and the compatibility user-data folder is trimmed to its cap while no surface is live.
- The subresource blocker fails closed and classifies requests by resource kind instead of treating every subresource as a generic one.
- Discarding or sleeping a tab releases its compatibility surface, and the composite 700/900 MB limits are applied to the memory budget while a compatibility tab is live.
- Reloading a compatibility tab reloads the Chromium document in place. Previously the reload re-ran the native fetch, whose result the compatibility surface never shows, so the visible page did not change.
- Single-threaded apartment initialisation is checked instead of ignored; a thread that is already multithreaded keeps the tab native.
- Reservation paths in the resource ledger deny rather than unwrap when a subsystem lookup is missing.
- Three clippy failures in the child IPC reader, the ledger, and the task manager are cleared, restoring the `-D warnings` gate.
- A compatibility tab now follows omnibox and in-page navigation: a live surface is driven by `ICoreWebView2::Navigate` for web targets and released when the tab turns to an internal page, instead of the native fetch silently running behind a blanked content area.
- Memory governance now charges the measured WebView2 process tree against the pressure budget, so widening the composite limit can no longer let Sylphra's own memory grow unchecked, and the `WebView` ledger booking is released when the last surface detaches so a stale reading cannot deny native reservations.
- The controller-cap is enforced across surfaces and pending requests together: the oldest pending request is retired at the cap, controller creation is refused once the outstanding limit is reached, and the recycling order is true least-recently-used (visibility churn no longer refreshes the recency stamp).
- The WebView2 environment is released when no surface or request remains, and a slow first-time environment creation no longer expires the requests that are waiting on it.
- A truncated subresource URI is now blocked rather than passed to the blocker in part, and user-data trimming is confined to the Chromium cache directories on a single directory walk, never touching profile state such as cookies, preferences, locks or storage.
- `windows_wv2` now declares the Win32 metadata features the compatibility host actually uses instead of relying on them being pulled in transitively.

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
