# Contributing to Sylphra

## Development workflow

1. Use the Rust toolchain defined in `rust-toolchain.toml`.
2. Keep changes inside the documented product and security boundaries.
3. Add unit tests for pure logic and integration tests for complete pipelines.
4. Run the release checks before submitting a change.

```powershell
cargo fmt --all -- --check
cargo check --locked
cargo test --lib --locked

# Centralized tiers with one build job and JSON metrics
.\tools\test.ps1 -Tier fast
.\tools\test.ps1 -Tier release
.\tools\test.ps1 -Tier full
```

Changes affecting parsing, layout, networking, storage or untrusted input should
also add a bounded regression test. Performance-sensitive changes
should run `cargo bench --locked` and report the before/after result.

## Code guidelines

- Keep production Rust safe; isolate and justify any future unsafe code.
- Prefer explicit size, time, depth and count limits for untrusted input.
- Do not expose a UI control until its end-to-end behavior and failure state are
  implemented and tested.
- Keep asynchronous responses bound to the originating tab and navigation sequence.
- Preserve incognito isolation and avoid persisting private state.
- Clean-room development: do not copy, translate or adapt code from another browser engine.
- Do not add a dependency until its license and release-artifact notices have been reviewed.

## Pull-request checklist

- Formatting, checks, tests and Clippy pass with the locked dependency graph.
- New behavior has deterministic tests that do not depend on public internet.
- Documentation, changelog and version metadata are consistent.
- No generated `target/` or `dist/` artifact is included.
- Security-sensitive changes explain their trust boundary and resource limits.

Please follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).
