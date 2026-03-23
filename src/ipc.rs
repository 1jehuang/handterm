use crate::backend::Backend;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub cmd: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn ok_empty() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

pub enum IpcAction {
    None,
    SendText {
        window: Option<u64>,
        bytes: Vec<u8>,
    },
    SetTitle {
        window: Option<u64>,
        title: String,
    },
    Close {
        window: Option<u64>,
    },
    OpenWindow {
        cols: Option<u16>,
        rows: Option<u16>,
    },
    FocusWindow(u64),
    SyntheticKeyEvent {
        window: Option<u64>,
        event: SyntheticKeyEvent,
    },
    SyntheticImeCommit {
        window: Option<u64>,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticKeyEvent {
    pub kind: crate::frontend::KeyEventKind,
    pub key: String,
    pub text: Option<String>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

pub struct IpcServer {
    listener: UnixListener,
    clients: Vec<IpcClient>,
    path: PathBuf,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

struct IpcClient {
    stream: UnixStream,
    recv_buf: Vec<u8>,
}

impl IpcServer {
    pub fn bind(path: &Path) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path).ok();
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let listener = UnixListener::bind(path)
            .with_context(|| format!("failed to bind IPC socket at {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("failed to set listener non-blocking")?;

        Ok(Self {
            listener,
            clients: Vec::new(),
            path: path.to_path_buf(),
            read_buf: vec![0u8; 8192],
            write_buf: Vec::with_capacity(4096),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub fn listener_fd(&self) -> BorrowedFd<'_> {
        self.listener.as_fd()
    }

    pub fn listener_raw_fd(&self) -> i32 {
        self.listener.as_raw_fd()
    }

    #[allow(dead_code)]
    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    pub fn poll(
        &mut self,
        handler: &mut dyn FnMut(&Request) -> (Response, IpcAction),
    ) -> Vec<IpcAction> {
        self.accept_new();

        if self.clients.is_empty() {
            return Vec::new();
        }

        let mut actions = Vec::new();
        let mut to_remove = Vec::new();

        for (i, client) in self.clients.iter_mut().enumerate() {
            let n = match nix::unistd::read(&client.stream, &mut self.read_buf) {
                Ok(0) => {
                    to_remove.push(i);
                    continue;
                }
                Ok(n) => n,
                Err(nix::errno::Errno::EAGAIN) => continue,
                Err(_) => {
                    to_remove.push(i);
                    continue;
                }
            };

            client.recv_buf.extend_from_slice(&self.read_buf[..n]);

            while let Some(newline_pos) = memchr_newline(&client.recv_buf) {
                let line = &client.recv_buf[..newline_pos];

                if let Ok(req) = serde_json::from_slice::<Request>(line) {
                    let (resp, action) = handler(&req);

                    self.write_buf.clear();
                    if serde_json::to_writer(&mut self.write_buf, &resp).is_ok() {
                        self.write_buf.push(b'\n');
                        if client.stream.write_all(&self.write_buf).is_err() {
                            to_remove.push(i);
                        }
                    }

                    match action {
                        IpcAction::None => {}
                        other => actions.push(other),
                    }
                }

                client.recv_buf.drain(..=newline_pos);
            }
        }

        for i in to_remove.into_iter().rev() {
            self.clients.swap_remove(i);
        }

        actions
    }

    fn accept_new(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    stream.set_nonblocking(true).ok();
                    self.clients.push(IpcClient {
                        stream,
                        recv_buf: Vec::with_capacity(512),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

#[inline]
fn memchr_newline(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == b'\n')
}

fn host_socket_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Cpu => "handterm-cpu.sock",
        Backend::Gpu => "handterm-gpu.sock",
    }
}

pub fn default_socket_path_for_backend(backend: Backend) -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join(host_socket_name(backend))
}

pub fn default_socket_path() -> PathBuf {
    default_socket_path_for_backend(Backend::Cpu)
}

pub fn find_socket_for_backend(backend: Backend) -> Option<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    let dir = PathBuf::from(&runtime_dir);

    let stable = dir.join(host_socket_name(backend));
    if stable.exists() {
        if UnixStream::connect(&stable).is_ok() {
            return Some(stable);
        }
        std::fs::remove_file(&stable).ok();
    }

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("handterm-") && name_str.ends_with(".sock") {
                let path = entry.path();
                if UnixStream::connect(&path).is_ok() {
                    return Some(path);
                }
                std::fs::remove_file(&path).ok();
            }
        }
    }
    None
}

pub fn find_socket() -> Option<PathBuf> {
    find_socket_for_backend(Backend::Cpu)
}

pub fn send_command(socket_path: &Path, req: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    let mut data = serde_json::to_vec(req).context("failed to serialize request")?;
    data.push(b'\n');
    stream.write_all(&data).context("failed to send request")?;

    let mut resp_buf = vec![0u8; 65536];
    let mut total = 0;
    loop {
        match nix::unistd::read(&stream, &mut resp_buf[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if resp_buf[..total].contains(&b'\n') {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let line = &resp_buf[..total];
    let resp: Response =
        serde_json::from_slice(line.trim_ascii()).context("failed to parse response")?;
    Ok(resp)
}
