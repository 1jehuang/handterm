use crate::protocol::{
    ClientMessage, ServerMessage, decode_server_message, read_server_message, write_client_message,
};
use anyhow::{Context, Result};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TryRecvStatus {
    Message(ServerMessage),
    Empty,
    Closed,
}

pub struct ProtocolClient {
    stream: UnixStream,
    recv_buf: Vec<u8>,
}

impl ProtocolClient {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
        Ok(Self {
            stream,
            recv_buf: Vec::with_capacity(4096),
        })
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        self.stream
            .set_nonblocking(nonblocking)
            .context("failed to set protocol client non-blocking")
    }

    pub fn raw_fd(&self) -> i32 {
        self.stream.as_raw_fd()
    }

    pub fn send(&mut self, message: &ClientMessage) -> Result<()> {
        write_client_message(&mut self.stream, message)
    }

    pub fn recv(&mut self) -> Result<ServerMessage> {
        read_server_message(&mut self.stream)
    }

    pub fn try_recv(&mut self) -> Result<TryRecvStatus> {
        loop {
            if let Some(frame) = take_frame(&mut self.recv_buf)? {
                return Ok(TryRecvStatus::Message(decode_server_message(&frame)?));
            }

            let mut io_buf = [0u8; 8192];
            match nix::unistd::read(&self.stream, &mut io_buf) {
                Ok(0) => return Ok(TryRecvStatus::Closed),
                Ok(n) => self.recv_buf.extend_from_slice(&io_buf[..n]),
                Err(nix::errno::Errno::EAGAIN) => return Ok(TryRecvStatus::Empty),
                Err(err) => return Err(err).context("failed reading protocol client"),
            }
        }
    }
}

fn take_frame(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > 8 * 1024 * 1024 {
        anyhow::bail!("client frame exceeds max size");
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    let payload = buf[4..4 + len].to_vec();
    buf.drain(..4 + len);
    Ok(Some(payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClientMessage, MouseButton, MouseEvent, MouseEventKind};
    use std::io::Write;

    #[test]
    fn protocol_client_over_unix_pair_roundtrips_frames() {
        let (mut server, client) = UnixStream::pair().expect("unix stream pair should open");
        let mut client = ProtocolClient {
            stream: client,
            recv_buf: Vec::new(),
        };

        let send = ClientMessage::MouseInput {
            window_id: 7,
            event: MouseEvent {
                kind: MouseEventKind::ScrollUp,
                button: MouseButton::None,
                col: 3,
                row: 2,
                modifiers: 0,
            },
        };

        client.send(&send).expect("client frame should send");
        let received = crate::protocol::read_client_message(&mut server)
            .expect("server should read framed client message");
        assert_eq!(received, send);

        let reply = ServerMessage::Bell { window_id: 7 };
        crate::protocol::write_server_message(&mut server, &reply)
            .expect("server frame should send");
        let echoed = client
            .recv()
            .expect("client should read framed server message");
        assert_eq!(echoed, reply);
    }

    #[test]
    fn protocol_client_try_recv_handles_partial_frames() {
        let (mut server, client) = UnixStream::pair().expect("unix stream pair should open");
        let mut client = ProtocolClient {
            stream: client,
            recv_buf: Vec::new(),
        };
        client
            .set_nonblocking(true)
            .expect("client stream should become non-blocking");

        let reply = ServerMessage::SetTitle {
            window_id: 4,
            title: "remote".to_string(),
        };
        let mut payload = Vec::new();
        crate::protocol::write_server_message(&mut payload, &reply)
            .expect("frame should encode into byte vec");

        server
            .write_all(&payload[..3])
            .expect("partial server write should succeed");
        assert_eq!(
            client.try_recv().expect("partial recv should succeed"),
            TryRecvStatus::Empty
        );

        server
            .write_all(&payload[3..])
            .expect("remaining server write should succeed");
        assert_eq!(
            client.try_recv().expect("complete recv should succeed"),
            TryRecvStatus::Message(reply)
        );
    }
}
