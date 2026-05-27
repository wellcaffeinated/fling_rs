use std::process::Stdio;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::protocol::{self, ServerAck, CH_ERROR, CH_EXIT, CH_STDIN, CH_STDIN_EOF, CH_STDERR, CH_STDOUT};

pub async fn run(socket_path: &str, config: Config) -> Result<()> {
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    let config = Arc::new(config);
    eprintln!("fling: listening on {socket_path}");

    loop {
        let (stream, _) = listener.accept().await?;
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, config).await {
                eprintln!("fling: connection error: {e}");
            }
        });
    }
}

async fn handle_connection(stream: tokio::net::UnixStream, config: Arc<Config>) -> Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();

    // Handshake: read request
    let request: protocol::ClientRequest = protocol::read_json_line(&mut read_half).await?;

    // Check allowlist
    let entry = match config.commands.get(&request.cmd) {
        Some(e) => e.clone(),
        None => {
            let ack = ServerAck {
                ok: false,
                error: Some(format!("command '{}' is not in the allowlist", request.cmd)),
            };
            protocol::write_json_line(&mut write_half, &ack).await?;
            return Ok(());
        }
    };

    protocol::write_json_line(&mut write_half, &ServerAck { ok: true, error: None }).await?;

    // Spawn subprocess
    let mut cmd = Command::new(&entry.executable);
    cmd.args(&request.args);
    if let Some(wd) = &entry.working_dir {
        cmd.current_dir(wd);
    }
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
