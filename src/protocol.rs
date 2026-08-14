use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Largest handshake line accepted, in bytes. Both peers send one JSON line
/// before any framing, from an unauthenticated connection — without a cap, a
/// client that never sends a newline grows the read buffer until the server
/// dies. Generous next to a realistic argv, which is chunked far below this.
pub const MAX_HANDSHAKE_BYTES: usize = 1024 * 1024;

/// Largest frame payload accepted, in bytes. The 4-byte length header is
/// attacker-controlled, so it must be bounded before it reaches an allocation.
/// Both peers write 8 KiB chunks, leaving 128x headroom.
pub const MAX_FRAME_PAYLOAD: usize = 1024 * 1024;

pub const CH_STDIN: u8     = 0x01;
pub const CH_STDIN_EOF: u8 = 0x02;
pub const CH_STDOUT: u8    = 0x11;
pub const CH_STDERR: u8    = 0x12;
pub const CH_EXIT: u8      = 0x13;
pub const CH_ERROR: u8     = 0x14;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientRequest {
    pub cmd: String,
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ServerAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header).await?;
    let channel = header[0];
    let len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
    // The peer controls this length, so bound it before it sizes an allocation.
    if len > MAX_FRAME_PAYLOAD {
        return Err(anyhow!(
            "frame payload of {len} bytes exceeds the {MAX_FRAME_PAYLOAD} byte limit"
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok((channel, payload))
}

pub async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    channel: u8,
    payload: &[u8],
) -> Result<()> {
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(channel);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    writer.write_all(&buf).await?;
    Ok(())
}

pub async fn read_json_line<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut buf = Vec::new();
    loop {
        let b = reader.read_u8().await?;
        if b == b'\n' {
            break;
        }
        // Bound the buffer: the newline may never arrive.
        if buf.len() >= MAX_HANDSHAKE_BYTES {
            return Err(anyhow!(
                "handshake line exceeds the {MAX_HANDSHAKE_BYTES} byte limit"
            ));
        }
        buf.push(b);
    }
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn write_json_line<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_oversized_frame_length() {
        // Five bytes of header can claim a 4 GiB payload. The length is
        // attacker-controlled and must be refused before it sizes a buffer.
        let mut header = vec![CH_STDIN];
        header.extend_from_slice(&u32::MAX.to_be_bytes());

        let err = read_frame(&mut &header[..]).await.unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "expected a size-limit error, got: {err}"
        );
    }

    #[tokio::test]
    async fn accepts_frame_within_limit() {
        let mut buf = Vec::new();
        write_frame(&mut buf, CH_STDOUT, b"hello").await.unwrap();

        let (channel, payload) = read_frame(&mut &buf[..]).await.unwrap();
        assert_eq!(channel, CH_STDOUT);
        assert_eq!(payload, b"hello");
    }

    #[tokio::test]
    async fn rejects_unterminated_handshake_line() {
        // A client that connects and streams bytes without ever sending a
        // newline must hit the cap, not exhaust memory.
        let flood = vec![b'x'; MAX_HANDSHAKE_BYTES + 1];

        let err = read_json_line::<_, ClientRequest>(&mut &flood[..])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "expected a size-limit error, got: {err}"
        );
    }
}
