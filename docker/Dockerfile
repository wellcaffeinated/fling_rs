# Build stage: compile a release binary (Rust 2024 edition needs rustc >= 1.85).
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# Runtime stage: a minimal image with fling plus the symlinks used to relay
# commands by name. Both the server and client containers run this same image.
FROM debian:bookworm-slim
COPY --from=build /src/target/release/fling /usr/local/bin/fling

# Symlink-mode commands: invoking `foo`/`relaycat`/`bar` relays that name.
RUN ln -s /usr/local/bin/fling /usr/local/bin/foo \
 && ln -s /usr/local/bin/fling /usr/local/bin/relaycat \
 && ln -s /usr/local/bin/fling /usr/local/bin/bar

COPY docker/config.toml /etc/fling/config.toml
COPY docker/smoke.sh /usr/local/bin/smoke.sh
RUN chmod +x /usr/local/bin/smoke.sh

CMD ["fling", "server", "--socket", "unix:/run/fling/fling.sock", "--config", "/etc/fling/config.toml"]
