use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const ENV_SOCKET: &str = "HANDTERM_NATIVE_SCROLL_SOCKET";
const ENV_WINDOW_ID: &str = "HANDTERM_WINDOW_ID";
const ENV_TERM_PROGRAM: &str = "TERM_PROGRAM";
const TERM_PROGRAM_VALUE: &str = "handterm";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    Chat,
    SidePanel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneState {
    pub kind: PaneKind,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub position: usize,
    pub content_length: usize,
    pub viewport_length: usize,
}

impl PaneState {
    pub fn contains(&self, col: usize, row: usize) -> bool {
        let x = usize::from(self.x);
        let y = usize::from(self.y);
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        col >= x && col < x.saturating_add(width) && row >= y && row < y.saturating_add(height)
    }

    pub fn scrollable(&self) -> bool {
        self.content_length > self.viewport_length && self.viewport_length > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PaneSnapshot {
    pub panes: Vec<PaneState>,
}

impl PaneSnapshot {
    pub fn hovered_pane(&self, col: usize, row: usize) -> Option<PaneKind> {
        self.panes
            .iter()
            .find(|pane| pane.scrollable() && pane.contains(col, row))
            .map(|pane| pane.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppToHost {
    PaneSnapshot { panes: Vec<PaneState> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToApp {
    Scroll { pane: PaneKind, delta: i32 },
}

#[derive(Debug)]
pub struct NativeScrollBridge {
    socket_path: PathBuf,
    snapshot: Arc<Mutex<PaneSnapshot>>,
    command_tx: Sender<HostToApp>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    chat_residual: f32,
    side_panel_residual: f32,
}

impl NativeScrollBridge {
    pub fn new(window_id: u64) -> Result<Self> {
        let socket_path = socket_path_for_window(window_id);
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let listener = UnixListener::bind(&socket_path).with_context(|| {
            format!(
                "failed binding native scroll socket {}",
                socket_path.display()
            )
        })?;
        listener
            .set_nonblocking(true)
            .context("failed setting native scroll listener nonblocking")?;

        let snapshot = Arc::new(Mutex::new(PaneSnapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = mpsc::channel();

        let thread = spawn_bridge_thread(listener, snapshot.clone(), stop.clone(), command_rx);

        Ok(Self {
            socket_path,
            snapshot,
            command_tx,
            stop,
            thread: Some(thread),
            chat_residual: 0.0,
            side_panel_residual: 0.0,
        })
    }

    pub fn child_envs(&self, window_id: u64) -> [(&'static str, String); 3] {
        [
            (ENV_SOCKET, self.socket_path.display().to_string()),
            (ENV_WINDOW_ID, window_id.to_string()),
            (ENV_TERM_PROGRAM, TERM_PROGRAM_VALUE.to_string()),
        ]
    }

    pub fn hovered_pane(&self, col: usize, row: usize) -> Option<PaneKind> {
        self.snapshot.lock().ok()?.hovered_pane(col, row)
    }

    pub fn send_scroll_delta(&mut self, pane: PaneKind, delta_rows: f32) -> bool {
        let residual = match pane {
            PaneKind::Chat => &mut self.chat_residual,
            PaneKind::SidePanel => &mut self.side_panel_residual,
        };
        *residual += delta_rows;

        let mut steps = 0i32;
        while *residual >= 1.0 {
            steps += 1;
            *residual -= 1.0;
        }
        while *residual <= -1.0 {
            steps -= 1;
            *residual += 1.0;
        }

        if steps == 0 {
            return true;
        }

        self.command_tx
            .send(HostToApp::Scroll { pane, delta: steps })
            .is_ok()
    }
}

impl Drop for NativeScrollBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn socket_path_for_window(window_id: u64) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!(
        "handterm-scroll-{}-{}.sock",
        std::process::id(),
        window_id
    ))
}

fn spawn_bridge_thread(
    listener: UnixListener,
    snapshot: Arc<Mutex<PaneSnapshot>>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<HostToApp>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("handterm-native-scroll".to_string())
        .spawn(move || bridge_thread(listener, snapshot, stop, command_rx))
        .expect("native scroll bridge thread should spawn")
}

fn bridge_thread(
    listener: UnixListener,
    snapshot: Arc<Mutex<PaneSnapshot>>,
    stop: Arc<AtomicBool>,
    command_rx: Receiver<HostToApp>,
) {
    let mut stream = None::<UnixStream>;
    let mut read_buf = Vec::<u8>::new();

    while !stop.load(Ordering::Relaxed) {
        if stream.is_none() {
            match listener.accept() {
                Ok((accepted, _)) => {
                    let _ = accepted.set_nonblocking(true);
                    stream = Some(accepted);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }

        if let Some(active) = stream.as_mut() {
            while let Ok(message) = command_rx.try_recv() {
                if write_line(active, &message).is_err() {
                    stream = None;
                    break;
                }
            }
        }

        if let Some(active) = stream.as_mut() {
            match read_messages(active, &mut read_buf) {
                Ok(messages) => {
                    for message in messages {
                        let AppToHost::PaneSnapshot { panes } = message;
                        if let Ok(mut current) = snapshot.lock() {
                            current.panes = panes;
                        }
                    }
                }
                Err(_) => {
                    stream = None;
                    if let Ok(mut current) = snapshot.lock() {
                        current.panes.clear();
                    }
                }
            }
        }

        thread::sleep(Duration::from_millis(8));
    }
}

fn read_messages(stream: &mut UnixStream, buffer: &mut Vec<u8>) -> Result<Vec<AppToHost>> {
    let mut chunk = [0u8; 4096];
    let mut messages = Vec::new();

    loop {
        match stream.read(&mut chunk) {
            Ok(0) => anyhow::bail!("native scroll peer closed"),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(err).context("failed reading native scroll stream"),
        }
    }

    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
        let line = buffer.drain(..=pos).collect::<Vec<_>>();
        let line = &line[..line.len().saturating_sub(1)];
        if line.is_empty() {
            continue;
        }
        let message = serde_json::from_slice::<AppToHost>(line)
            .context("failed decoding native scroll message")?;
        messages.push(message);
    }

    Ok(messages)
}

fn write_line<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).context("failed encoding native scroll message")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .context("failed writing native scroll command")
}

pub fn child_socket_path() -> Option<PathBuf> {
    std::env::var_os(ENV_SOCKET).map(PathBuf::from)
}

pub fn child_window_id() -> Option<u64> {
    std::env::var(ENV_WINDOW_ID).ok()?.parse().ok()
}

pub fn socket_env_key() -> &'static str {
    ENV_SOCKET
}

pub fn term_program_env_key() -> &'static str {
    ENV_TERM_PROGRAM
}

pub fn term_program_env_value() -> &'static str {
    TERM_PROGRAM_VALUE
}

pub fn window_env_key() -> &'static str {
    ENV_WINDOW_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_hit_testing_uses_cell_rect() {
        let snapshot = PaneSnapshot {
            panes: vec![PaneState {
                kind: PaneKind::Chat,
                x: 2,
                y: 3,
                width: 10,
                height: 4,
                position: 0,
                content_length: 20,
                viewport_length: 4,
            }],
        };
        assert_eq!(snapshot.hovered_pane(2, 3), Some(PaneKind::Chat));
        assert_eq!(snapshot.hovered_pane(11, 6), Some(PaneKind::Chat));
        assert_eq!(snapshot.hovered_pane(12, 6), None);
    }

    #[test]
    fn scroll_delta_accumulates_fractional_rows() {
        let mut bridge = NativeScrollBridge::new(999_991).expect("bridge should initialize");
        let _ = bridge.send_scroll_delta(PaneKind::Chat, 0.4);
        assert!((bridge.chat_residual - 0.4).abs() < f32::EPSILON);
        let _ = bridge.send_scroll_delta(PaneKind::Chat, 0.7);
        assert!(bridge.chat_residual >= 0.0 && bridge.chat_residual < 1.0);
    }
}
