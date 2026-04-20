use crate::config::AppConfig;
use crate::protocol::{
    ClientMessage, ServerMessage, WindowId, decode_client_message, encode_server_message,
    read_server_message, write_client_message,
};
use crate::pty::PtyChild;
use crate::server::{ServerCore, ServerIoAction};
use anyhow::{Context, Result};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

const SERVER_POLL_TIMEOUT_MS: u16 = 50;
const CLIENT_IO_BUF_SIZE: usize = 64 * 1024;
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(2);

fn daemon_style_defaults() -> (String, f64) {
    let defaults = AppConfig::default();
    (defaults.style.font_family, defaults.style.font_size)
}

pub fn default_server_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join("handterm-server.sock")
}

pub fn run_server_only_with_build_id(
    socket: Option<PathBuf>,
    config: &AppConfig,
    protocol_build_id: String,
) -> Result<()> {
    let socket_path = socket.unwrap_or_else(default_server_socket_path);
    let mut daemon = ServerDaemon::bind(&socket_path, config.scrollback.lines, protocol_build_id)?;
    eprintln!("handterm server listening on {}", socket_path.display());
    daemon.run()
}

pub fn ensure_server_running_with_build_id(
    socket_path: &Path,
    config_override: Option<&Path>,
    protocol_build_id: &str,
) -> Result<()> {
    if server_is_compatible(socket_path, protocol_build_id) {
        return Ok(());
    }

    if socket_path.exists() {
        std::fs::remove_file(socket_path).ok();
    }

    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server-only").arg("--socket").arg(socket_path);
    if let Some(config_path) = config_override {
        cmd.arg("--config").arg(config_path);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn server for {}", socket_path.display()))?;

    let deadline = Instant::now() + SERVER_START_TIMEOUT;
    while Instant::now() < deadline {
        if server_is_compatible(socket_path, protocol_build_id) {
            return Ok(());
        }
        if let Some(status) = child.try_wait().context("failed polling spawned server")? {
            anyhow::bail!("server exited early with status {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    anyhow::bail!(
        "timed out waiting for server socket {}",
        socket_path.display()
    );
}

fn server_is_compatible(socket_path: &Path, protocol_build_id: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    let probe = ClientMessage::Ping {
        build_id: protocol_build_id.to_string(),
    };
    if write_client_message(&mut stream, &probe).is_err() {
        return false;
    }
    matches!(
        read_server_message(&mut stream),
        Ok(ServerMessage::Pong { build_id }) if build_id == protocol_build_id
    )
}

struct ServerClient {
    stream: UnixStream,
    recv_buf: Vec<u8>,
    send_buf: Vec<u8>,
    windows: BTreeSet<WindowId>,
}

pub struct ServerDaemon {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<ServerClient>,
    core: ServerCore,
    ptys: BTreeMap<WindowId, PtyChild>,
    io_buf: Vec<u8>,
}

impl ServerDaemon {
    pub fn bind(path: &Path, scrollback_limit: usize, protocol_build_id: String) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path).ok();
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let listener = UnixListener::bind(path)
            .with_context(|| format!("failed to bind server socket at {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("failed to set server listener non-blocking")?;

        Ok(Self {
            listener,
            path: path.to_path_buf(),
            clients: Vec::new(),
            core: {
                let (font_family, font_size) = daemon_style_defaults();
                ServerCore::new_with_style(
                    scrollback_limit,
                    font_family,
                    font_size,
                    protocol_build_id,
                )
            },
            ptys: BTreeMap::new(),
            io_buf: vec![0; CLIENT_IO_BUF_SIZE],
        })
    }

    pub fn run(&mut self) -> Result<()> {
        loop {
            self.poll_once()?;
        }
    }

    fn poll_once(&mut self) -> Result<()> {
        let mut client_map = Vec::with_capacity(self.clients.len());
        let mut pty_map = Vec::with_capacity(self.ptys.len());
        let listener_fd = self.listener.as_fd();
        let mut poll_fds = vec![PollFd::new(listener_fd, PollFlags::POLLIN)];

        for (index, client) in self.clients.iter().enumerate() {
            let mut flags = PollFlags::POLLIN;
            if !client.send_buf.is_empty() {
                flags |= PollFlags::POLLOUT;
            }
            poll_fds.push(PollFd::new(client.stream.as_fd(), flags));
            client_map.push((poll_fds.len() - 1, index));
        }

        for (&window_id, pty) in &self.ptys {
            poll_fds.push(PollFd::new(pty.fd(), PollFlags::POLLIN));
            pty_map.push((poll_fds.len() - 1, window_id));
        }

        poll(&mut poll_fds, PollTimeout::from(SERVER_POLL_TIMEOUT_MS))
            .context("daemon poll failed")?;

        let listener_ready = poll_fds[0]
            .revents()
            .unwrap_or(PollFlags::empty())
            .contains(PollFlags::POLLIN);
        let mut client_readable = Vec::new();
        let mut client_writable = Vec::new();
        let mut client_dead = Vec::new();
        let mut pty_readable = Vec::new();

        for (poll_index, client_index) in client_map {
            let flags = poll_fds[poll_index].revents().unwrap_or(PollFlags::empty());
            if flags.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
                client_dead.push(client_index);
                continue;
            }
            if flags.contains(PollFlags::POLLIN) {
                client_readable.push(client_index);
            }
            if flags.contains(PollFlags::POLLOUT) {
                client_writable.push(client_index);
            }
        }

        for (poll_index, window_id) in pty_map {
            let flags = poll_fds[poll_index].revents().unwrap_or(PollFlags::empty());
            if flags.contains(PollFlags::POLLIN) {
                pty_readable.push(window_id);
            }
        }
        drop(poll_fds);

        if listener_ready {
            self.accept_new()?;
        }

        for index in client_writable {
            if self.flush_client(index).is_err() {
                client_dead.push(index);
            }
        }

        for index in client_readable {
            if self.read_client(index).is_err() {
                client_dead.push(index);
            }
        }

        client_dead.sort_unstable();
        client_dead.dedup();
        for index in client_dead.into_iter().rev() {
            self.disconnect_client(index);
        }

        for window_id in pty_readable {
            self.poll_pty(window_id)?;
        }

        Ok(())
    }

    fn accept_new(&mut self) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_nonblocking(true)
                        .context("failed to set client stream non-blocking")?;
                    self.clients.push(ServerClient {
                        stream,
                        recv_buf: Vec::with_capacity(4096),
                        send_buf: Vec::with_capacity(4096),
                        windows: BTreeSet::new(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err).context("failed accepting daemon client"),
            }
        }
        Ok(())
    }

    fn flush_client(&mut self, client_index: usize) -> Result<()> {
        let Some(client) = self.clients.get_mut(client_index) else {
            return Ok(());
        };
        while !client.send_buf.is_empty() {
            match nix::unistd::write(&client.stream, &client.send_buf) {
                Ok(0) => anyhow::bail!("client closed during write"),
                Ok(n) => {
                    client.send_buf.drain(..n);
                }
                Err(nix::errno::Errno::EAGAIN) => break,
                Err(err) => return Err(err).context("failed writing daemon client"),
            }
        }
        Ok(())
    }

    fn read_client(&mut self, client_index: usize) -> Result<()> {
        loop {
            let n = {
                let client = self
                    .clients
                    .get_mut(client_index)
                    .context("invalid client index")?;
                match nix::unistd::read(&client.stream, &mut self.io_buf) {
                    Ok(0) => anyhow::bail!("client disconnected"),
                    Ok(n) => {
                        client.recv_buf.extend_from_slice(&self.io_buf[..n]);
                        n
                    }
                    Err(nix::errno::Errno::EAGAIN) => 0,
                    Err(err) => return Err(err).context("failed reading daemon client"),
                }
            };

            if n == 0 {
                break;
            }

            while let Some(frame) = take_frame(
                &mut self
                    .clients
                    .get_mut(client_index)
                    .context("invalid client index")?
                    .recv_buf,
            )? {
                let message = decode_client_message(&frame)?;
                self.handle_client_message(client_index, message)?;
            }
        }
        Ok(())
    }

    fn handle_client_message(&mut self, client_index: usize, message: ClientMessage) -> Result<()> {
        let is_new_window = matches!(message, ClientMessage::NewWindow { .. });
        let close_window_id = match &message {
            ClientMessage::CloseWindow { window_id } => Some(*window_id),
            _ => None,
        };
        let result = self
            .core
            .handle_client_message(message)
            .map_err(|err| anyhow::anyhow!("server rejected client message: {err:?}"))?;

        let maybe_window_id = result.messages.iter().find_map(|message| match message {
            ServerMessage::WindowCreated { window_id, .. } => Some(*window_id),
            _ => None,
        });

        if is_new_window && let Some(window_id) = maybe_window_id {
            self.clients
                .get_mut(client_index)
                .context("invalid client index")?
                .windows
                .insert(window_id);
        }

        if let Some(window_id) = close_window_id
            && let Some(client) = self.clients.get_mut(client_index)
        {
            client.windows.remove(&window_id);
        }

        for action in result.io_actions {
            self.apply_io_action(action)?;
        }
        for message in result.messages {
            self.queue_client_message(client_index, &message)?;
        }
        self.flush_client(client_index)?;
        Ok(())
    }

    fn apply_io_action(&mut self, action: ServerIoAction) -> Result<()> {
        match action {
            ServerIoAction::SpawnWindow {
                window_id,
                cols,
                rows,
            } => {
                let pty = PtyChild::spawn_default_shell(cols, rows)
                    .with_context(|| format!("failed to spawn PTY for window {window_id}"))?;
                self.ptys.insert(window_id, pty);
            }
            ServerIoAction::Write { window_id, bytes } => {
                if let Some(pty) = self.ptys.get(&window_id) {
                    pty.write_all(&bytes)?;
                }
            }
            ServerIoAction::Resize {
                window_id,
                cols,
                rows,
            } => {
                if let Some(pty) = self.ptys.get(&window_id) {
                    pty.resize(cols, rows)?;
                }
            }
            ServerIoAction::Close { window_id } => {
                self.ptys.remove(&window_id);
            }
        }
        Ok(())
    }

    fn poll_pty(&mut self, window_id: WindowId) -> Result<()> {
        let mut closed = false;
        let mut accumulated = Vec::new();

        if let Some(pty) = self.ptys.get(&window_id) {
            loop {
                match pty.try_read(&mut self.io_buf) {
                    Ok(0) => break,
                    Ok(n) => accumulated.extend_from_slice(&self.io_buf[..n]),
                    Err(_) => {
                        closed = true;
                        break;
                    }
                }
            }
        }

        if !accumulated.is_empty()
            && let Some(result) = self.core.process_output(window_id, &accumulated)
        {
            for action in result.io_actions {
                self.apply_io_action(action)?;
            }
            self.broadcast_window_messages(window_id, &result.messages)?;
        }

        if closed {
            self.ptys.remove(&window_id);
            if let Some(message) = self.core.close_window(window_id, None) {
                self.broadcast_window_messages(window_id, &[message])?;
            }
        }

        Ok(())
    }

    fn broadcast_window_messages(
        &mut self,
        window_id: WindowId,
        messages: &[ServerMessage],
    ) -> Result<()> {
        if let Some(client_index) = self.client_index_for_window(window_id) {
            for message in messages {
                self.queue_client_message(client_index, message)?;
            }
            self.flush_client(client_index)?;
        }
        Ok(())
    }

    fn queue_client_message(&mut self, client_index: usize, message: &ServerMessage) -> Result<()> {
        let encoded = encode_server_frame(message)?;
        let client = self
            .clients
            .get_mut(client_index)
            .context("invalid client index")?;
        client.send_buf.extend_from_slice(&encoded);
        Ok(())
    }

    fn client_index_for_window(&self, window_id: WindowId) -> Option<usize> {
        self.clients
            .iter()
            .position(|client| client.windows.contains(&window_id))
    }

    fn disconnect_client(&mut self, client_index: usize) {
        if client_index >= self.clients.len() {
            return;
        }
        let client = self.clients.swap_remove(client_index);
        for window_id in client.windows {
            self.ptys.remove(&window_id);
            let _ = self.core.close_window(window_id, None);
        }
    }
}

impl Drop for ServerDaemon {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

fn encode_server_frame(message: &ServerMessage) -> Result<Vec<u8>> {
    let payload = encode_server_message(message)?;
    let len = u32::try_from(payload.len()).context("server frame too large")?;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn take_frame(buf: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > 8 * 1024 * 1024 {
        anyhow::bail!("daemon frame exceeds max size");
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
    use crate::client::ProtocolClient;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn take_frame_waits_for_complete_payload() {
        let message = ServerMessage::Bell { window_id: 7 };
        let encoded = encode_server_frame(&message).expect("frame should encode");
        let mut partial = encoded[..encoded.len() - 1].to_vec();

        assert!(
            take_frame(&mut partial)
                .expect("partial frame parse should succeed")
                .is_none()
        );

        partial.push(*encoded.last().expect("frame should have a last byte"));
        let payload = take_frame(&mut partial)
            .expect("complete frame parse should succeed")
            .expect("complete frame should decode");
        let decoded =
            crate::protocol::decode_server_message(&payload).expect("server message should decode");
        assert_eq!(decoded, message);
        assert!(partial.is_empty());
    }

    #[test]
    fn take_frame_rejects_oversized_payloads() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(8 * 1024 * 1024u32 + 1).to_le_bytes());
        buf.extend_from_slice(&[0u8; 16]);

        assert!(take_frame(&mut buf).is_err());
    }

    #[test]
    fn default_server_socket_path_uses_runtime_dir() {
        let path = default_server_socket_path();
        assert!(path.ends_with("handterm-server.sock"));
    }

    #[test]
    fn daemon_bind_uses_configured_scrollback_limit() {
        let temp = tempdir().expect("tempdir should exist");
        let socket_path = temp.path().join("handterm-server.sock");
        let mut daemon = ServerDaemon::bind(&socket_path, 0, "test-build".to_string())
            .expect("daemon should bind");

        let created = daemon.core.create_window(4, 2, 96);
        let window_id = match created {
            ServerMessage::WindowCreated { window_id, .. } => window_id,
            other => panic!("expected WindowCreated, got {other:?}"),
        };
        daemon
            .core
            .process_output(window_id, b"abcdefghij")
            .expect("window should exist");

        assert_eq!(daemon.core.window_scrollback_limit(window_id), Some(0));
        assert_eq!(daemon.core.window_scrollback_len(window_id), Some(0));
    }

    #[test]
    fn daemon_roundtrip_new_window_and_close_window() {
        let temp = tempdir().expect("tempdir should exist");
        let socket_path = temp.path().join("handterm-server.sock");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let handle = thread::spawn(move || {
            let mut daemon = ServerDaemon::bind(
                &socket_path,
                crate::grid::DEFAULT_SCROLLBACK_MAX,
                "test-build".to_string(),
            )
            .expect("daemon should bind");
            while !stop_thread.load(Ordering::Relaxed) {
                daemon.poll_once().expect("daemon poll should succeed");
            }
        });

        let mut client = None;
        for _ in 0..50 {
            match ProtocolClient::connect(temp.path().join("handterm-server.sock").as_path()) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        let mut client = client.expect("client should connect to daemon");

        client
            .send(&ClientMessage::NewWindow {
                cols: 20,
                rows: 5,
                dpi: 96,
            })
            .expect("new window should send");
        let created = client.recv().expect("window created should arrive");
        let window_id = match created {
            ServerMessage::WindowCreated {
                window_id,
                cols,
                rows,
                ..
            } => {
                assert_eq!((cols, rows), (20, 5));
                window_id
            }
            other => panic!("expected WindowCreated, got {other:?}"),
        };

        client
            .send(&ClientMessage::CloseWindow { window_id })
            .expect("close window should send");
        let mut closed = None;
        for _ in 0..16 {
            let message = client.recv().expect("daemon reply should arrive");
            if let ServerMessage::WindowClosed { .. } = message {
                closed = Some(message);
                break;
            }
        }
        let closed = closed.expect("window closed should arrive");
        assert_eq!(
            closed,
            ServerMessage::WindowClosed {
                window_id,
                exit_code: None,
            }
        );

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("daemon thread should join");
    }

    #[test]
    fn compatible_server_probe_succeeds_for_live_daemon() {
        let temp = tempdir().expect("tempdir should exist");
        let socket_path = temp.path().join("handterm-server.sock");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let socket_path_for_thread = socket_path.clone();

        let handle = thread::spawn(move || {
            let mut daemon = ServerDaemon::bind(
                &socket_path_for_thread,
                crate::grid::DEFAULT_SCROLLBACK_MAX,
                "test-build".to_string(),
            )
            .expect("daemon should bind");
            while !stop_thread.load(Ordering::Relaxed) {
                daemon.poll_once().expect("daemon poll should succeed");
            }
        });

        for _ in 0..50 {
            if server_is_compatible(&socket_path, "test-build") {
                stop.store(true, Ordering::Relaxed);
                handle.join().expect("daemon thread should join");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("daemon thread should join");
        panic!("live daemon should satisfy compatibility probe");
    }

    #[test]
    fn compatible_server_probe_rejects_mismatched_build_id() {
        let temp = tempdir().expect("tempdir should exist");
        let socket_path = temp.path().join("handterm-server.sock");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let socket_path_for_thread = socket_path.clone();

        let handle = thread::spawn(move || {
            let mut daemon = ServerDaemon::bind(
                &socket_path_for_thread,
                crate::grid::DEFAULT_SCROLLBACK_MAX,
                "server-build".to_string(),
            )
            .expect("daemon should bind");
            while !stop_thread.load(Ordering::Relaxed) {
                daemon.poll_once().expect("daemon poll should succeed");
            }
        });

        for _ in 0..50 {
            if socket_path.exists() {
                assert!(!server_is_compatible(&socket_path, "client-build"));
                stop.store(true, Ordering::Relaxed);
                handle.join().expect("daemon thread should join");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("daemon thread should join");
        panic!("daemon socket should appear for mismatch probe test");
    }
}
