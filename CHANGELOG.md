# Changelog

All notable changes to Sylphra are documented here. The project follows Semantic Versioning.

## [0.0.0-alpha] - 2026-09-22

### Added
- Comprehensive project architecture re-engineered into 8 domain modules (`core`, `engine`, `net`, `media`, `storage`, `ui`, `platform`, `experimental`).
- Full integration test suite restructured into 7 domain suites matching `src/` modules (`engine`, `net`, `media`, `storage`, `ui`, `platform`, `acceptance`).
- Native Windows packaging pipeline with Inno Setup installer (`Sylphra-v0.0.0-alpha-Setup.exe`) and portable zip distribution.
- Safe Rust document-focused browser pipeline without embedding Chromium, Gecko or WebKit.
