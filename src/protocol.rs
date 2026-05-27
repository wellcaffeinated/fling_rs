use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CH_STDIN: u8     = 0x01;
pub const CH_STDIN_EOF: u8 = 0x02;
pub const CH_STDOUT: u8    = 0x11;
pub const CH_STDERR: u8    = 0x12;
pub const CH_EXIT: u8      = 0x13;
pub const CH_ERROR: u8     = 0x14;

#[derive(Serialize, Deserialize)]
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
