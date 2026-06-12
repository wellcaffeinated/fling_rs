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
fling server --socket unix:/run/fling/fling.sock --config /etc/fling/config.toml
```

| Flag | Default | Description |
|---|---|---|
| `--socket` / `-s` | `unix:/run/fling/fling.sock` | Socket path (`unix:/path` or bare path) |
| `--config` / `-c` | `/etc/fling/config.toml` | Config file |

### Client

There are three equivalent ways to relay a command, in order of preference:

```sh
# 1. Symlinked by command name (socket from $FLING_SOCKET).
ln -s /usr/local/bin/fling /usr/local/bin/obsidian
FLING_SOCKET=unix:/run/fling/fling.sock obsidian --vault notes

# 2. Explicit, with --socket (or $FLING_SOCKET).
fling --socket unix:/run/fling/fling.sock obsidian --vault notes

# 3. Implicit client mode: anything that isn't `server`.
fling obsidian --vault notes   # uses $FLING_SOCKET or the default socket
```

`fling` without the `server` subcommand is always client mode. The socket
defaults to `unix:/run/fling/fling.sock` and can be set with `$FLING_SOCKET`.

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
name itself is implied by the entry), using the [`globset`](https://docs.rs/globset)
crate — so `*`, `?`, `[a-z]` character classes and `{a,b}` alternation all work.
`*` matches across any character, including spaces and `/`. Patterns are
compiled and validated when the server starts, so a malformed glob fails fast.

A command with no `allow` patterns (or an unknown command name) is rejected. The
client prints exactly:

```
You are not authorized to execute this command
```

and exits 1. The same message is used whether the command is unknown or merely
its arguments are disallowed, so the rules don't reveal which commands exist;
the server logs the specific reason to its own stderr. So `obsidian create`
would be denied above, while `obsidian --vault notes` is allowed.

## Filesystem sandboxing (optional)

A command can be confined with [bubblewrap](https://github.com/containers/bubblewrap)
(`bwrap`) so it only sees an explicit set of paths. This is how you restrict,
say, a relayed `cat` to one directory:

```toml
[commands.cat]
executable = "/bin/cat"
allow      = ["*"]

[commands.cat.sandbox]
ro_bind = ["/srv/share"]      # read-only paths
rw_bind = ["/srv/uploads"]    # read-write paths (optional)
proc    = true                # mount a private /proc (default true)
dev     = true                # mount a minimal /dev  (default true)
```

`cat /srv/share/notes.txt` works; `cat /etc/passwd` fails with *No such file* —
the file isn't in the sandbox at all, independent of the glob rules. Each
sandboxed command runs in fresh mount/pid/user/network namespaces with only
`/usr`, the runtime libraries, a fresh `/tmp`, optionally `/proc` and `/dev`,
plus the bound paths. Set `proc = false` / `dev = false` for commands that don't
need them (e.g. `cat`).

### Does this need root?

No. bubblewrap is built for exactly this — **an unprivileged user can sandbox
commands with no setuid, no capabilities, no root.** On any modern host with
unprivileged user namespaces enabled (the default on Debian, Ubuntu, Arch and
Fedora; check with `sysctl kernel.unprivileged_userns_clone` or
`/proc/sys/user/max_user_namespaces`), running the fling server as a normal user
is all you need. Just make sure `bwrap` is on its `PATH`.

The one place it gets fiddly is running bwrap *nested inside another sandbox*
that restricts the namespace syscalls — most notably a default-configured Docker
container, whose seccomp profile blocks `unshare`/`clone` with `CLONE_NEWUSER`,
and whose masked `/proc` blocks mounting a fresh procfs. That's a property of
the outer container, not of fling. See the smoke-test notes below for how the
compose file handles it without going privileged. Commands without a
`[*.sandbox]` section run unsandboxed as before.

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

The client container drives symlinked commands (`foo`, `relaycat`, `safecat`,
`bar`) and asserts the access rules hold — including that the sandboxed
`safecat` can read its bound directory but not `/etc/passwd`. It exits 0 when
every check passes. Tear down with `docker compose -f docker/compose.yml down -v`.

The compose file gives the server container `security_opt: [seccomp=unconfined]`
— *not* `privileged` — which is the minimal grant that lets bubblewrap create
namespaces inside Docker (no extra capabilities, no device access). The demo's
`safecat` also sets `proc = false`, since Docker's masked `/proc` blocks mounting
a fresh procfs unprivileged. On a normal host neither tweak is needed.
