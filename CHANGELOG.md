# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/wellcaffeinated/fling_rs/compare/v0.1.0...v0.2.0) - 2026-08-14

### Breaking

- Commands are now confined with bubblewrap unless they set `sandbox = false`.
  A command with no `[*.sandbox]` section previously saw the whole filesystem;
  it now sees only `/usr`, the runtime libraries and `/tmp` until paths are
  granted with `ro_bind`/`rw_bind`.
- Relayed commands no longer inherit the server process's working directory.
  Sandboxed commands start in an empty `/tmp`, unsandboxed ones in `/`, so
  relative paths that resolved against the server's launch directory now fail.
- Setting `working_dir` now also grants read access to that directory inside
  the sandbox, and becomes the command's working directory.
- `bwrap` must be installed wherever the server runs. Without it, sandboxed
  commands fail rather than running unconfined.
- The default socket path is now `/run/fling/fling.sock`.

### Added

- Symlink-name relay: invoking the binary under any name other than `fling`
  relays that name as the command, forwarding all arguments verbatim.
- Default-deny access rules, with per-command `allow` glob patterns compiled
  by `globset` at startup.
- Bubblewrap sandboxing with `ro_bind`/`rw_bind` grants and optional `proc`
  and `dev` mounts.
- Loud startup warning when `bwrap` is missing while commands are sandboxed,
  silenced with `warn_missing_bwrap = false`.
- The server now creates the socket's parent directory if it doesn't exist.

### Fixed

- Bound allocations driven by untrusted peer input: frame payloads and
  handshake lines are capped at 1 MiB. A 5-byte frame header could previously
  request a 4 GiB allocation, and a client that never sent a newline could
  grow the handshake buffer without limit.
- Connection pressure no longer kills the server: `accept` failures are logged
  and retried rather than terminating the process, connections that don't
  complete a handshake within 10s are dropped, and concurrent connections are
  capped. Exhausting the server's file descriptors previously stopped the
  relay permanently.
- Client output is flushed before exit, fixing intermittently truncated output.
- Denials return a uniform message, so access rules don't reveal which
  commands exist.
- Spawn failures name the program that actually failed rather than the relayed
  executable, which was misleading when `bwrap` was the missing one.

### Documentation

- README rewritten around defaults, with generic examples, prebuilt-binary
  install, and a glob "gotchas" section covering path traversal and the
  surprises in pattern matching.

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
