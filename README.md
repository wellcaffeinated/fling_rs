# fling

A Unix socket command relay. A **server** exposes an explicit allowlist of permitted commands; a **client** connects and executes them with stdin/stdout/stderr forwarded verbatim.

## Use case

Run `fling` in server mode inside one container, and place a thin wrapper script in another:

```sh
#!/bin/sh
exec fling --socket unix:/run/obsidian.sock obsidian "$@"
```

The wrapper behaves exactly like running `obsidian` directly — piped input, exit codes, stderr — but the binary never leaves the server container.

## Installation

```sh
cargo build --release
# binary at target/release/fling
```

## Usage

### Server

```sh
fling server --socket unix:/run/fling.sock --config /etc/fling/config.toml
```

| Flag | Default | Description |
|---|---|---|
| `--socket` / `-s` | `unix:/run/fling.sock` | Socket path (`unix:/path` or bare path) |
| `--config` / `-c` | `/etc/fling/config.toml` | Config file |

### Client

```sh
fling --socket unix:/run/fling.sock <command> [args...]
```

`fling` without the `server` subcommand is always client mode.

## Config

```toml
[commands.obsidian]
executable  = "/usr/local/bin/obsidian-headless"
working_dir = "/home/agent"   # optional

[commands.convert]
executable = "/usr/bin/convert"
```

Only commands listed here can be executed. Any other command name is rejected with an error and exit code 1.

## Protocol

The connection is split into two phases:

1. **Handshake** — JSON lines: client sends `{"cmd":"…","args":[…]}`, server replies `{"ok":true}` or `{"ok":false,"error":"…"}`.
2. **Streaming** — binary frames: `[1-byte channel][4-byte big-endian length][payload]`.

| Channel | Direction | Meaning |
|---|---|---|
| `0x01` | client → server | stdin chunk |
| `0x02` | client → server | stdin EOF |
| `0x11` | server → client | stdout chunk |
| `0x12` | server → client | stderr chunk |
| `0x13` | server → client | exit code (4-byte i32) |
| `0x14` | server → client | server error string |

## Tests

```sh
cargo test
```

Covers: basic relay, stdin forwarding, binary round-trips, exit code propagation, stderr separation, hyphenated args, 1 MB output, 10 concurrent clients, disallowed commands.
