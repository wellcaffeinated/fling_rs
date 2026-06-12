# fling — developer notes

## Build & test

```sh
cargo build
cargo build --release

# cargo test can time out in this container on first run (dependency compilation).
# Preferred approach:
cargo test --no-run                          # compile tests
./target/debug/deps/integration-*[^.d] 2>&1 # run directly
```

If `cargo fetch` hasn't been run yet, do that first — crates.io downloads can be slow.

## Module layout

| File | Role |
|---|---|
| `src/main.rs` | Entry point. Two dispatch checks before clap: (1) symlink mode — if `argv[0]`'s basename isn't `fling`, that name is the command to relay and all args are forwarded; socket comes from `$FLING_SOCKET`. (2) implicit client mode — if `argv[1]` isn't `server`, prepend `client`. |
| `src/cli.rs` | clap structs. `Client` subcommand is internal; users omit it. `--socket` reads `$FLING_SOCKET` and defaults to `unix:/run/fling/fling.sock`. |
| `src/config.rs` | TOML config loading + `Config::authorize` (default-deny). Allow globs are compiled to a `globset::GlobSet` per command at load (fail-fast on bad patterns). Read once at startup, wrapped in `Arc<Config>`. |
| `src/sandbox.rs` | `build_command` — returns the `tokio::process::Command` to spawn, wrapping it in `bwrap` when the command has a `[*.sandbox]` section. |
| `src/protocol.rs` | Wire format: `read_frame`/`write_frame` for binary frames, `read_json_line`/`write_json_line` for the handshake. |
| `src/server.rs` | Accept loop + per-connection handler. Each connection spawns 4 tasks (A: stdin relay, B: stdout, C: stderr, D: socket writer). |
| `src/client.rs` | Connects, sends request, relays stdin (task), receives output frames (task). |
| `tests/integration.rs` | Integration tests. Each test starts a real server subprocess and exercises the full binary. `start_with_rules` sets per-command allow globs. |
| `docker/` | Two-container smoke test (`compose.yml`, `Dockerfile`, `config.toml`, `smoke.sh`). |

## Access-rule model

- **Default-deny**: a request is authorized only if `config.commands` contains the command name *and* one of that command's `allow` globs matches the space-joined arguments. See `Config::authorize`.
- **Patterns match args only** — the command name is implied by the config key. `allow = ["*"]` permits any arguments; an absent/empty `allow` denies everything.
- Globs use the `globset` crate (`*`, `?`, `[...]`, `{a,b}`); `*` matches across `/` (literal_separator defaults off). Compiled at load.
- **Uniform denial**: `Config::authorize` returns a detailed `Err` reason which the server logs to its own stderr (`fling: denied: …`), but the client only ever receives `"You are not authorized to execute this command"` via `ServerAck{ok:false}`. The client prints that message verbatim (no `fling:` prefix) and exits 1. Don't leak the specific reason to the client.

## Sandboxing

- A command with a `[commands.<name>.sandbox]` section (`ro_bind` / `rw_bind` lists) is spawned via `bwrap` — see `src/sandbox.rs`. Only `/usr`, runtime lib dirs, a private `/proc`, `/dev`, `/tmp`, and the bound paths are visible; namespaces (incl. network) are unshared.
- This is how a relayed `cat` is confined to a directory: the unbound files don't exist in the sandbox, independent of the glob rules.
- Requires `bwrap` on PATH and unprivileged user namespaces. The docker smoke test runs the server `privileged` only because it nests namespaces inside Docker.

## Protocol invariants

- **Handshake first**: one JSON line each direction before any binary frames.
- **Ordering**: server always sends all `Stdout`/`Stderr` frames before the `Exit` frame. The client relies on this for correct output capture.
- **Exit is terminal**: after sending `Exit` or `Error`, the server closes the connection.
- **Stdin EOF**: client sends `CH_STDIN_EOF` (0x02) when its stdin closes. Server closes the child's stdin pipe on receipt.

## Concurrency model (server, per connection)

```
socket ReadHalf → Task A (stdin relay) → child stdin pipe
child stdout   → Task B → mpsc tx_b ─┐
child stderr   → Task C → mpsc tx_c ─┴→ Task D (writer) → socket WriteHalf
```

Tasks B and C drop their `tx` clones when done; Task D exits when the channel drains. After `join!(B, C, D)`, the server aborts Task A and sends the Exit frame directly.

## Integration test design

- Each test gets a unique socket path and config file (`/tmp/fling-test-{id}.*`).
- `TestServer::start` waits for readiness by **connecting**, not just checking file existence — the socket file appears during `bind()`, before `accept()` is running.
- Tests run in parallel by default; the connection-based readiness check makes this safe.

## Common pitfalls

- **Don't check socket file existence for readiness** — use an actual connect attempt (see `TestServer::start`).
- **`std::process::exit` in client main** — intentional, propagates the remote exit code exactly. Don't replace with `?` propagation. **But** because `process::exit` does *not* flush, the client's output task must `flush()` tokio stdout/stderr before returning, or relayed output is intermittently truncated (was a real, load-dependent bug).
- **Stale socket files** — server calls `remove_file` at startup. If the server crashes without cleanup, restart removes it automatically.
- **Edition 2024** — this project uses Rust 2024 edition (requires rustc ≥ 1.85).
