# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/wellcaffeinated/fling_rs/compare/v0.1.0...v0.1.1) - 2026-08-14

### Other

- Document glob gotchas
- Default sandboxed CWD to /tmp instead of a synthetic dir
- Confine commands and pin the working directory by default
- Assert intended sandbox defaults in unit tests
- Keep connection pressure from killing the server
- Bound allocations driven by untrusted peer input
- Rewrite README: surface defaults, generic examples, release install
- make /proc and /dev optional; drop privileged from smoke test
- Default socket to /run/fling/fling.sock
- Use globset, uniform denial message, bwrap sandboxing; fix output flush
- Add symlink-name relay and default-deny glob access rules

## [0.1.0](https://github.com/wellcaffeinated/fling_rs/releases/tag/v0.1.0) - 2026-05-27

### Other

- Configure release-plz for git-only mode
- add verbose logging to release-plz PR job for debugging
- Add release-plz workflow and disable crates.io publishing
- Fix macOS Intel runner label (macos-13 → macos-15-intel)
- Add GitHub release workflow for Linux musl and macOS
- Add CLAUDE.md with developer guidance
- Add README
- Initial implementation of fling
