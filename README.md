# Sylphra Browser

Sylphra is a lightweight, Windows-first browser written in safe Rust, focusing on bounded resource usage, security isolation, and high performance.

This project is a new development iteration built upon the foundation of [GhitaBrowser](https://github.com/ghitatruongle/GhitaBrowser). You can explore the original predecessor repository at [https://github.com/ghitatruongle/GhitaBrowser](https://github.com/ghitatruongle/GhitaBrowser).

## Key Highlights

- Safe Rust Core: Strict adherence to safe Rust and deterministic bounded memory ceilings.
- Process Isolation: Multi-process architecture separating browser coordination from renderer workers with automatic crash recovery.
- Flexible Rendering: Native bounded layout and DOM engine with support for Chromium-based WebLite runtime for modern web compatibility and fallback display.
- Privacy and Security: Built-in ad-blocking, tracking protection, cosmetic filtering, and partitioned storage.
- Native Windows Integration: Native tab strip, sleeping and discarded tabs, reader mode, PDF viewer, and Task Manager.

## Quick Start

### Prerequisites

- Windows x64
- Rust toolchain (stable)

### Build and Run

```powershell
cargo build --release --locked
cargo run --release --locked
```

### Verification and Tests

```powershell
$env:CARGO_BUILD_JOBS = "1"
$env:RUST_MIN_STACK = "33554432"

cargo check --all-targets --locked
cargo test --all-targets --locked -j 1
```

## Essential Shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+L / F6 | Focus address bar |
| Ctrl+T / Ctrl+W | Open / close tab |
| Ctrl+Shift+T | Restore last closed tab |
| Ctrl+Tab | Next tab |
| Ctrl+Shift+A | Search open tabs |
| Shift+Esc | Task Manager |
| Ctrl+H / Ctrl+J | History / Downloads |
| Ctrl+O | Open local file |
| F12 | Diagnostics panel |

## Project Origin

Sylphra represents the next-generation evolution of the browser architecture originally prototyped in [GhitaBrowser](https://github.com/ghitatruongle/GhitaBrowser). The codebase has been refactored for clean multi-process boundaries, self-documenting code, and modern WebLite backend integration.

## License

See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for licensing details.
