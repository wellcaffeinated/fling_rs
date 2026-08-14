use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::protocol::{self, ServerAck, CH_ERROR, CH_EXIT, CH_STDIN, CH_STDIN_EOF, CH_STDERR, CH_STDOUT};

/// How long a connection may sit without completing its handshake. Without a
/// bound, connections opened and never written to are held indefinitely, and
/// enough of them exhaust the server's file descriptors.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum connections served at once. Each one costs a socket, four tasks and
/// (once authorized) a child process, so this bounds the resources any set of
/// clients can tie up.
const MAX_CONNECTIONS: usize = 128;

/// Pause after a failed `accept`, so a persistent failure (typically file
/// descriptor exhaustion) doesn't spin the accept loop at full speed.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

pub async fn run(socket_path: &str, config: Config) -> Result<()> {
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    let config = Arc::new(config);
    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    eprintln!("fling: listening on {socket_path}");

    loop {
        // Acquiring first bounds how many connections we hold at once: past the
        // limit we simply stop accepting, and the kernel queues or refuses on
        // our behalf. This also bounds concurrent child processes.
        let permit = connections
            .clone()
            .acquire_owned()
            .await
            .expect("connection semaphore is never closed");

        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                // Accept failures are transient — running out of file
                // descriptors must not take the whole relay down. Back off
                // briefly so a persistent error doesn't spin the loop.
                eprintln!("fling: accept failed: {e}");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };

        let config = config.clone();
        tokio::spawn(async move {
            let _permit = permit; // released when the connection finishes
            if let Err(e) = handle_connection(stream, config, HANDSHAKE_TIMEOUT).await {
                eprintln!("fling: connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    config: Arc<Config>,
    handshake_timeout: Duration,
) -> Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();

    // Handshake: read request. Bounded in time so a connection that never
    // speaks is dropped instead of held open.
    let request: protocol::ClientRequest =
        tokio::time::timeout(handshake_timeout, protocol::read_json_line(&mut read_half))
            .await
            .map_err(|_| anyhow!("handshake timed out after {handshake_timeout:?}"))??;

    // Authorize under the default-deny access rules (command must be configured
    // and its arguments must match an allow glob). The detailed reason is logged
    // server-side; the client only ever sees a uniform denial, so the rules
    // don't leak which commands exist.
    let entry = match config.authorize(&request.cmd, &request.args) {
        Ok(e) => e.clone(),
        Err(reason) => {
            eprintln!("fling: denied: {reason}");
            let ack = ServerAck {
                ok: false,
                error: Some("You are not authorized to execute this command".to_string()),
            };
            protocol::write_json_line(&mut write_half, &ack).await?;
            return Ok(());
        }
    };

    protocol::write_json_line(&mut write_half, &ServerAck { ok: true, error: None }).await?;

    // Spawn subprocess (wrapped in a bwrap sandbox if the command configures one).
    let mut cmd = crate::sandbox::build_command(&entry, &request.args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("failed to spawn '{}': {e}", entry.executable);
            let mut frame = Vec::with_capacity(5 + msg.len());
            frame.push(CH_ERROR);
            frame.extend_from_slice(&(msg.len() as u32).to_be_bytes());
            frame.extend_from_slice(msg.as_bytes());
            write_half.write_all(&frame).await?;
            return Ok(());
        }
    };

    let mut child_stdin = child.stdin.take().unwrap();
    let mut child_stdout = child.stdout.take().unwrap();
    let mut child_stderr = child.stderr.take().unwrap();

    // mpsc channel serializes all outbound frames through one writer
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    let tx_b = tx.clone();
    let tx_c = tx.clone();
    drop(tx); // channel closes when tx_b and tx_c are both dropped

    // Task A: relay stdin frames from socket → child stdin
    let task_a = tokio::spawn(async move {
        loop {
            let (channel, payload) = match protocol::read_frame(&mut read_half).await {
                Ok(f) => f,
                Err(_) => break,
            };
            match channel {
                CH_STDIN => {
                    if child_stdin.write_all(&payload).await.is_err() {
                        break;
                    }
                }
                CH_STDIN_EOF => break,
                _ => {}
            }
        }
        // dropping child_stdin closes child's stdin pipe
    });

    // Task B: child stdout → outbound frames
    let task_b = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match child_stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut frame = Vec::with_capacity(5 + n);
            frame.push(CH_STDOUT);
            frame.extend_from_slice(&(n as u32).to_be_bytes());
            frame.extend_from_slice(&buf[..n]);
            if tx_b.send(frame).await.is_err() {
                break;
            }
        }
    });

    // Task C: child stderr → outbound frames
    let task_c = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match child_stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut frame = Vec::with_capacity(5 + n);
            frame.push(CH_STDERR);
            frame.extend_from_slice(&(n as u32).to_be_bytes());
            frame.extend_from_slice(&buf[..n]);
            if tx_c.send(frame).await.is_err() {
                break;
            }
        }
    });

    // Task D: drain outbound frame queue → socket write half
    let task_d = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if write_half.write_all(&frame).await.is_err() {
                break;
            }
        }
        write_half
    });

    // Wait for stdout/stderr relay and writer to all finish
    let (_, _, write_half_result) = tokio::join!(task_b, task_c, task_d);
    let mut write_half = write_half_result?;

    task_a.abort();

    let status = child.wait().await?;
    let code = status.code().unwrap_or(-1);

    // Send exit frame: 1-byte channel + 4-byte length (=4) + 4-byte i32
    let mut frame = Vec::with_capacity(9);
    frame.push(CH_EXIT);
    frame.extend_from_slice(&4u32.to_be_bytes());
    frame.extend_from_slice(&code.to_be_bytes());
    write_half.write_all(&frame).await?;
    write_half.flush().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Arc<Config> {
        Arc::new(crate::config::parse_for_test(
            r#"
            [commands.echo]
            executable = "/bin/echo"
            allow = ["*"]
            "#,
        ))
    }

    #[tokio::test]
    async fn idle_connection_is_dropped_after_handshake_timeout() {
        // A client that connects and never speaks must not hold the connection
        // open indefinitely — that's what makes fd exhaustion cheap.
        let (server_side, mut client_side) = tokio::net::UnixStream::pair().unwrap();
        tokio::spawn(handle_connection(
            server_side,
            test_config(),
            Duration::from_millis(50),
        ));

        let mut buf = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), client_side.read(&mut buf)).await;

        let n = read
            .expect("server held an idle connection past the handshake timeout")
            .unwrap();
        assert_eq!(n, 0, "expected the server to close the connection");
    }

    #[tokio::test]
    async fn handshake_within_timeout_is_served() {
        // The timeout must not disturb a client that does speak up.
        let (server_side, mut client_side) = tokio::net::UnixStream::pair().unwrap();
        tokio::spawn(handle_connection(
            server_side,
            test_config(),
            Duration::from_secs(5),
        ));

        client_side
            .write_all(b"{\"cmd\":\"echo\",\"args\":[\"hi\"]}\n")
            .await
            .unwrap();

        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(5), client_side.read(&mut buf))
            .await
            .expect("server never acknowledged the handshake")
            .unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("\"ok\":true"),
            "expected an ack, got: {}",
            String::from_utf8_lossy(&buf[..n])
        );
    }
}
