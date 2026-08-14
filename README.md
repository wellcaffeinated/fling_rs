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

Socket paths accept `unix:/path` or a bare `/path`. The server creates the socket's parent directory if it doesn't exist.

Symlink mode forwards every argument to the remote command, so it has no `--socket` flag; it reads `$FLING_SOCKET`, falling back to the default.

Per-command config defaults:

| Key | Default | Effect |
|---|---|---|
| `allow` | *(empty)* | Denies every invocation |
| `sandbox` | *(unset)* | **Sandboxed** with no paths granted — set `sandbox = false` to opt out |
| `working_dir` | *(unset)* | Starts in the sandbox's own empty `/tmp`; nothing on the host is reachable by relative path |
| `sandbox.ro_bind` / `rw_bind` | *(empty)* | No host paths visible beyond the runtime image |
| `sandbox.proc` / `dev` | `true` | Private `/proc` and minimal `/dev` mounted |

Top-level settings:

| Key | Default | Effect |
|---|---|---|
| `warn_missing_bwrap` | `true` | Warn loudly at startup if `bwrap` isn't on `PATH` while commands are sandboxed |

## Config

```toml
[commands.mytool]
executable  = "/opt/mytool/bin/mytool"
working_dir = "/srv/work"           # optional; also grants access to it
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

#### Gotchas

Patterns are matched as **plain text** against the arguments joined by single spaces. There is no path logic, no shell, and no notion of "one argument". That leads to a few surprises:

| You write | You might expect | What actually happens |
|---|---|---|
| `allow = ["/srv/data/*"]` | only files under `/srv/data` | also matches `/srv/data/../../etc/passwd` — it's text, not a path. **Use the sandbox for real containment.** |
| `allow = ["list*"]` | `list`, maybe one flag | any trailing text: `list --all --force …`. `*` spans spaces *and* `/` |
| `allow = ["list *"]` | `list` works | bare `list` is **denied** — the pattern demands that space |
| `allow = ["status"]` | `status` plus flags | only exactly `status`; `status --force` is denied |
| `allow = ["--filter [a-z]"]` | the literal text `[a-z]` | matches `--filter a` — brackets are a character class |
| `allow = ["/data/**"]` | deeper than `*` | identical to `/data/*`; `*` already crosses `/` |
| `allow = ["run"]` | covers bare `run` | zero arguments join to `""`, so bare `run` needs `allow = [""]` |

Three more, briefly:

- **Matching is case-sensitive.** `STATUS` does not match `status`.
- **There are no deny rules.** You can't allow `list*` *except* `list --secrets`; if a pattern matches, it's permitted.
- **A typo takes the server down.** Patterns compile at startup, so one malformed glob anywhere is a startup failure, not a per-command problem.

To match a literal `[`, `?`, `*` or `{`, escape it *and* use a TOML literal (single-quoted) string, or TOML will eat the backslash first:

```toml
allow = ['--filter \[a-z\]']    # correct: literal brackets
allow = ["--filter \[a-z\]"]    # wrong: TOML consumes the backslash
```

### Sandboxing

**Every command is confined by default.** Commands run through [bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) in fresh mount/pid/user/network namespaces containing `/usr`, the runtime library directories, a fresh `/tmp`, `/proc` and `/dev` — and nothing else. A command with no `sandbox` section can read no host files at all, and starts in the sandbox's own `/tmp` — a fresh tmpfs, empty and writable, discarded when the command exits — so relative paths reach nothing on the host either. (A command with `sandbox = false` starts in `/` instead, since there the host's shared `/tmp` would be a poor default.)

Granting access is explicit:

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

`cat /srv/share/notes.txt` works; `cat /etc/passwd` fails with *No such file* — the file isn't in the sandbox at all, independent of the glob rules.

Setting `working_dir` is itself a grant: the directory is bound read-only into the sandbox (unless an existing bind already covers it) and becomes the command's CWD.

To run a command unconfined, say so:

```toml
[commands.trusted]
executable = "/usr/bin/tool"
allow      = ["*"]
sandbox    = false           # full filesystem access
```

`bwrap` must be on the server's `PATH`. If it isn't while commands are sandboxed, the server warns loudly at startup and those commands fail rather than silently running unconfined; set `warn_missing_bwrap = false` to silence the warning.

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

Covers basic relay, stdin forwarding, binary round-trips, exit codes, stderr separation, hyphenated args, 1 MB output, 10 concurrent clients, glob rules, symlink invocation, sandbox defaults, frame/handshake size limits, and survival under a connection flood.

### Docker smoke test

Two containers — a server and a client — sharing a socket:

```sh
docker compose -f docker/compose.yml up --build \
    --abort-on-container-exit --exit-code-from client
docker compose -f docker/compose.yml down -v
```

The client drives symlinked commands (`foo`, `relaycat`, `safecat`, `bar`) and asserts the access rules hold, including that sandboxed `safecat` reads its bound directory but not `/etc/passwd`. Exits 0 when every check passes.

The server container gets `security_opt: [seccomp=unconfined]` — *not* `privileged` — the minimal grant that lets bubblewrap create namespaces inside Docker. The demo commands also set `proc = false`, since Docker's masked `/proc` blocks mounting a fresh procfs unprivileged. Neither is needed on a normal host.
