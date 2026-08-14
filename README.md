# fling

A Unix socket command relay. A **server** exposes a default-deny set of commands, gated by per-argument glob rules. A **client** runs them with stdin/stdout/stderr and the exit code forwarded verbatim.

The typical use is cross-container: the server runs where the tool is installed, and other containers invoke it over a shared socket without ever holding the binary.

## Install

### Prebuilt binary

Static Linux (x86-64, ARM64) and macOS (Intel, Apple Silicon) builds, with checksums, are on the [releases page](https://github.com/wellcaffeinated/fling_rs/releases).

### From source

Requires Rust 1.85+ (edition 2024).

```sh
cargo build --release   # binary at target/release/fling
```

## Quick start

Server:

```sh
fling server --socket unix:/run/fling/fling.sock --config /etc/fling/config.toml
```

Client — three equivalent forms:

```sh
# 1. Symlinked by command name (preferred). The link name is the command.
ln -s /usr/local/bin/fling /usr/local/bin/mytool
FLING_SOCKET=unix:/run/fling/fling.sock mytool --flag arg

# 2. Explicit socket.
fling --socket unix:/run/fling/fling.sock mytool --flag arg

# 3. Implicit client mode: any first argument other than `server`.
fling mytool --flag arg
```

Anything that isn't `fling server` is client mode.

## Defaults

| Setting | Default | Where |
|---|---|---|
| Server socket | `unix:/run/fling/fling.sock` | `--socket` (the server does not read `$FLING_SOCKET`) |
| Client socket | `unix:/run/fling/fling.sock` | `--socket` > `$FLING_SOCKET` > default |
| Config file | `/etc/fling/config.toml` | `--config` |

Socket paths accept `unix:/path` or a bare `/path`. The socket's **parent directory must already exist** — the server does not create it. Under systemd, `RuntimeDirectory=fling` handles that.

Symlink mode forwards every argument to the remote command, so it has no `--socket` flag; it reads `$FLING_SOCKET`, falling back to the default.

Per-command config defaults:

| Key | Default | Effect |
|---|---|---|
| `allow` | *(empty)* | Denies every invocation |
| `working_dir` | *(unset)* | Command inherits the **server process's** working directory |
| `sandbox` | *(unset)* | No sandbox — the command sees the whole filesystem |
| `sandbox.ro_bind` / `rw_bind` | *(empty)* | No host paths visible beyond the runtime image |
| `sandbox.proc` / `dev` | `true` | Private `/proc` and minimal `/dev` mounted |

## Config

```toml
[commands.mytool]
executable  = "/opt/mytool/bin/mytool"
working_dir = "/srv/work"           # optional
allow       = ["--flag *", "--version"]

[commands.git]
executable = "/usr/bin/git"
allow      = ["status*", "log*"]    # `git push` is denied

[commands.convert]
executable = "/usr/bin/convert"
allow      = ["*"]                  # any arguments
```

### Access rules

Policy is **default-deny**: a command runs only if it is configured *and* its arguments match one of its `allow` patterns.

- Patterns match the **space-joined arguments**; the command name is implied by the config key.
- Syntax is [`globset`](https://docs.rs/globset): `*`, `?`, `[a-z]`, `{a,b}`. `*` matches any character, including spaces and `/`.
- Patterns are compiled at startup, so a malformed glob fails immediately.
- No `allow` key, or an empty one, denies everything.

Every denial returns the same message, whether the command is unknown or its arguments are disallowed, so the rules don't reveal which commands exist:

```
You are not authorized to execute this command
```

The client prints exactly that and exits 1. The server logs the specific reason to its own stderr.

### Sandboxing

A `[commands.<name>.sandbox]` section confines the command with [bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) so it sees only the paths you bind:

```toml
[commands.cat]
executable = "/bin/cat"
allow      = ["*"]

[commands.cat.sandbox]
ro_bind = ["/srv/share"]     # read-only
rw_bind = ["/srv/uploads"]   # read-write
proc    = false              # default true
dev     = false              # default true
```

`cat /srv/share/notes.txt` works; `cat /etc/passwd` fails with *No such file* — the file isn't in the sandbox at all, independent of the glob rules. Sandboxed commands get fresh mount/pid/user/network namespaces containing `/usr`, the runtime library directories, a fresh `/tmp`, optionally `/proc` and `/dev`, and the bound paths. Commands with no `sandbox` section are not confined.

**Root is not required.** bubblewrap is designed for unprivileged use: no setuid, no capabilities. Any host with unprivileged user namespaces enabled (the default on Debian, Ubuntu, Arch, Fedora — check `/proc/sys/user/max_user_namespaces`) needs nothing beyond `bwrap` on the server's `PATH`.

Nesting bwrap inside *another* sandbox is the fiddly case. A default-configured Docker container blocks the namespace syscalls via seccomp and blocks a fresh procfs mount via its masked `/proc`. That's a property of the outer container, not fling — see the smoke test below for the minimal workaround.

## Protocol

1. **Handshake** — one JSON line each way: client sends `{"cmd":"…","args":[…]}`, server replies `{"ok":true}` or `{"ok":false,"error":"…"}`.
2. **Streaming** — binary frames: `[1-byte channel][4-byte big-endian length][payload]`.

| Channel | Direction | Meaning |
|---|---|---|
| `0x01` | client → server | stdin chunk |
| `0x02` | client → server | stdin EOF |
| `0x11` | server → client | stdout chunk |
| `0x12` | server → client | stderr chunk |
| `0x13` | server → client | exit code (4-byte i32) |
| `0x14` | server → client | server error string |

All stdout/stderr frames precede the exit frame, which is terminal.

## Tests

```sh
cargo test
```

Covers basic relay, stdin forwarding, binary round-trips, exit codes, stderr separation, hyphenated args, 1 MB output, 10 concurrent clients, glob rules, and symlink invocation.

### Docker smoke test

Two containers — a server and a client — sharing a socket:

```sh
docker compose -f docker/compose.yml up --build \
    --abort-on-container-exit --exit-code-from client
docker compose -f docker/compose.yml down -v
```

The client drives symlinked commands (`foo`, `relaycat`, `safecat`, `bar`) and asserts the access rules hold, including that sandboxed `safecat` reads its bound directory but not `/etc/passwd`. Exits 0 when every check passes.

The server container gets `security_opt: [seccomp=unconfined]` — *not* `privileged` — the minimal grant that lets bubblewrap create namespaces inside Docker. `safecat` also sets `proc = false`, since Docker's masked `/proc` blocks mounting a fresh procfs unprivileged. Neither is needed on a normal host.
