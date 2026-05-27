use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::protocol::{self, ClientRequest, ServerAck, CH_ERROR, CH_EXIT, CH_STDIN, CH_STDIN_EOF, CH_STDERR, CH_STDOUT};

pub async fn run(socket_path: &str, cmd: &str, args: &[String]) -> Result<i32> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect to {}: {}", socket_path, e))?;

    let (mut read_half, mut write_half) = stream.into_split();

    // Handshake: send request, receive ack
    protocol::write_json_line(&mut write_half, &ClientRequest {
        cmd: cmd.to_string(),
        args: args.to_vec(),
    })
    .await?;

    let ack: ServerAck = protocol::read_json_line(&mut read_half).await?;
    if !ack.ok {
        let msg = ack.error.unwrap_or_else(|| "unknown server error".to_string());
        eprintln!("fling: {msg}");
        return Ok(1);
    }

    // Task 1: local stdin → server stdin frames
    let task_stdin = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if protocol::write_frame(&mut write_half, CH_STDIN, &buf[..n])
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = protocol::write_frame(&mut write_half, CH_STDIN_EOF, &[]).await;
        let _ = write_half.flush().await;
    });

    // Task 2: server frames → local stdout/stderr
    let task_output = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();

        loop {
            let (channel, payload) = match protocol::read_frame(&mut read_half).await {
                Ok(f) => f,
                Err(_) => {
                    eprintln!("fling: connection closed unexpectedly");
                    return 1i32;
                }
            };

            match channel {
                CH_STDOUT => {
                    let _ = stdout.write_all(&payload).await;
                }
                CH_STDERR => {
                    let _ = stderr.write_all(&payload).await;
                }
                CH_EXIT => {
                    if payload.len() >= 4 {
                        return i32::from_be_bytes(payload[..4].try_into().unwrap());
                    }
                    return 0;
                }
                CH_ERROR => {
                    eprintln!("fling: {}", String::from_utf8_lossy(&payload));
                    return 1;
                }
                _ => {}
            }
        }
    });

    let exit_code = task_output.await?;
    task_stdin.abort();

    Ok(exit_code)
}
