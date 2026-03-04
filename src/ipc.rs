use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
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
    SendText(Vec<u8>),
    SetTitle(String),
    Close,
}

pub struct IpcServer {
    listener: UnixListener,
    clients: Vec<IpcClient>,
    path: PathBuf,
}

struct IpcClient {
    reader: BufReader<UnixStream>,
    write_stream: UnixStream,
    buf: String,
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
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn poll(&mut self, handler: &mut dyn FnMut(&Request) -> (Response, IpcAction)) -> Vec<IpcAction> {
        self.accept_new();

        let mut actions = Vec::new();
        let mut to_remove = Vec::new();

        for (i, client) in self.clients.iter_mut().enumerate() {
            match client.try_read_request() {
                Ok(Some(req)) => {
                    let (resp, action) = handler(&req);
                    if client.send_response(&resp).is_err() {
                        to_remove.push(i);
                    }
                    match action {
                        IpcAction::None => {}
                        other => actions.push(other),
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    to_remove.push(i);
                }
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
                    let write_stream = stream.try_clone().unwrap();
                    self.clients.push(IpcClient {
                        reader: BufReader::new(stream),
                        write_stream,
                        buf: String::with_capacity(4096),
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

impl IpcClient {
    fn try_read_request(&mut self) -> Result<Option<Request>> {
        self.buf.clear();
        match self.reader.read_line(&mut self.buf) {
            Ok(0) => anyhow::bail!("client disconnected"),
            Ok(_) => {
                let req: Request = serde_json::from_str(self.buf.trim())
                    .context("invalid JSON request")?;
                Ok(Some(req))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn send_response(&mut self, resp: &Response) -> Result<()> {
        let mut data = serde_json::to_vec(resp).context("failed to serialize response")?;
        data.push(b'\n');
        self.write_stream
            .write_all(&data)
            .context("failed to write response")?;
        Ok(())
    }
}

pub fn default_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join(format!("handterm-{}.sock", std::process::id()))
}

pub fn find_socket() -> Option<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let dir = PathBuf::from(&runtime_dir);

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

pub fn send_command(socket_path: &Path, req: &Request) -> Result<Response> {
    let stream = UnixStream::connect(socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    let mut writer = stream.try_clone().context("failed to clone stream")?;
    let mut data = serde_json::to_vec(req).context("failed to serialize request")?;
    data.push(b'\n');
    writer
        .write_all(&data)
        .context("failed to send request")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read response")?;
    let resp: Response =
        serde_json::from_str(line.trim()).context("failed to parse response")?;
    Ok(resp)
}
