# fling

A Unix socket command relay. A **server** exposes a default-deny set of permitted commands, gated by per-argument glob rules; a **client** connects and executes them with stdin/stdout/stderr forwarded verbatim.

## Use case

Run `fling` in server mode inside one container, and expose individual commands in another by **symlinking the binary** under the command's name:

```sh
ln -s /usr/local/bin/fling /usr/local/bin/obsidian
export FLING_SOCKET=unix:/run/obsidian.sock
obsidian "$@"   # relays the `obsidian` command to the server
```

When `fling` is invoked under any name other than `fling`, that name *is* the command it relays, and every argument is forwarded verbatim. The symlink behaves exactly like running `obsidian` directly — piped input, exit codes, stderr — but the binary never leaves the server container. (The older explicit form, `fling --socket unix:/run/obsidian.sock obsidian "$@"`, still works too.)

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

There are three equivalent ways to relay a command, in order of preference:

```sh
# 1. Symlinked by command name (socket from $FLING_SOCKET).
ln -s /usr/local/bin/fling /usr/local/bin/obsidian
FLING_SOCKET=unix:/run/fling.sock obsidian --vault notes

# 2. Explicit, with --socket (or $FLING_SOCKET).
fling --socket unix:/run/fling.sock obsidian --vault notes

# 3. Implicit client mode: anything that isn't `server`.
fling obsidian --vault notes   # uses $FLING_SOCKET or the default socket
```

`fling` without the `server` subcommand is always client mode. The socket
defaults to `unix:/run/fling.sock` and can be set with `$FLING_SOCKET`.

## Config & access rules

Policy is **default-deny**. A command runs only if it is configured *and* its
arguments match one of the command's `allow` glob patterns.

```toml
[commands.obsidian]
executable  = "/usr/local/bin/obsidian-headless"
working_dir = "/home/agent"        # optional
allow       = ["--vault *", "--version"]

[commands.git]
executable = "/usr/bin/git"
allow      = ["status*", "log*"]   # `git status`/`git log` ok; `git push` denied

[commands.convert]
executable = "/usr/bin/convert"
allow      = ["*"]                 # any arguments permitted
```

Each pattern is matched against the **space-joined arguments** (the command
name itself is implied by the entry). Wildcards:

| Token | Matches |
|---|---|
| `*` | any run of characters, including spaces and `/` |
| `?` | exactly one character |

A command with no `allow` patterns (or an unknown command name) is rejected with
an error and exit code 1. So `obsidian create` would be denied above, while
`obsidian --vault notes` is allowed.

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

Covers: basic relay, stdin forwarding, binary round-trips, exit code propagation, stderr separation, hyphenated args, 1 MB output, 10 concurrent clients, glob access rules, and symlink-name invocation.

### Docker smoke test

A two-container end-to-end test relays commands between a server and client
container over a shared socket:

```sh
docker compose -f docker/compose.yml up --build \
    --abort-on-container-exit --exit-code-from client
```

The client container drives symlinked commands (`foo`, `relaycat`, `bar`) and
asserts the access rules hold; it exits 0 when every check passes. Tear down
with `docker compose -f docker/compose.yml down -v`.
