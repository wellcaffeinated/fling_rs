# Build stage: compile a release binary (Rust 2024 edition needs rustc >= 1.85).
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# Runtime stage: a minimal image with fling, bubblewrap (for sandboxed commands),
# and the symlinks used to relay commands by name. Both the server and client
# containers run this same image.
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends bubblewrap \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/fling /usr/local/bin/fling

# Symlink-mode commands: invoking `foo`/`relaycat`/`safecat`/`bar` relays that name.
RUN ln -s /usr/local/bin/fling /usr/local/bin/foo \
 && ln -s /usr/local/bin/fling /usr/local/bin/relaycat \
 && ln -s /usr/local/bin/fling /usr/local/bin/safecat \
 && ln -s /usr/local/bin/fling /usr/local/bin/bar

# Data used by the sandboxed `safecat` demo: only /data/public is bound into the
# sandbox, so /data/secret.txt (and the rest of the host fs) stays invisible.
RUN mkdir -p /data/public \
 && echo 'public data' > /data/public/hello.txt \
 && echo 'TOP SECRET' > /data/secret.txt

COPY docker/config.toml /etc/fling/config.toml
COPY docker/smoke.sh /usr/local/bin/smoke.sh
RUN chmod +x /usr/local/bin/smoke.sh

CMD ["fling", "server", "--socket", "unix:/run/fling/fling.sock", "--config", "/etc/fling/config.toml"]
