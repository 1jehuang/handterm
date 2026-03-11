use crate::ipc::{IpcAction, Request, Response};
use crate::terminal::Terminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::event_loop::EventLoopProxy;

pub fn handle_ipc_request(terminal: &mut Terminal, req: &Request) -> (Response, IpcAction) {
    let target_window = req
        .args
        .as_object()
        .and_then(|o| o.get("window_id"))
        .and_then(|v| v.as_u64());

    match req.cmd.as_str() {
        "get-text" => {
            let text = if let Some(obj) = req.args.as_object() {
                let start = obj
                    .get("start_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let end = obj
                    .get("end_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(terminal.grid.rows as u64) as usize;
                terminal.grid.get_text(start, end)
            } else {
                terminal.grid.get_all_text()
            };
            (
                Response::ok(serde_json::json!({ "text": text })),
                IpcAction::None,
            )
        }
        "send-text" => {
            let text = req
                .args
                .as_object()
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                (Response::err("missing 'text' argument"), IpcAction::None)
            } else {
                (
                    Response::ok_empty(),
                    IpcAction::SendText {
                        window: target_window,
                        bytes: text.as_bytes().to_vec(),
                    },
                )
            }
        }
        "send-key" => {
            let key = req
                .args
                .as_object()
                .and_then(|o| o.get("key"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let bytes = match key {
                "enter" | "return" => Some(b"\r".to_vec()),
                "tab" => Some(b"\t".to_vec()),
                "escape" | "esc" => Some(b"\x1b".to_vec()),
                "backspace" => Some(b"\x7f".to_vec()),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                "home" => Some(b"\x1b[H".to_vec()),
                "end" => Some(b"\x1b[F".to_vec()),
                "delete" => Some(b"\x1b[3~".to_vec()),
                "page_up" => Some(b"\x1b[5~".to_vec()),
                "page_down" => Some(b"\x1b[6~".to_vec()),
                "space" => Some(b" ".to_vec()),
                k if k.starts_with("ctrl+") && k.len() == 6 => {
                    let ch = k.as_bytes()[5];
                    if ch.is_ascii_alphabetic() {
                        Some(vec![ch.to_ascii_lowercase() - b'a' + 1])
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match bytes {
                Some(bytes) => (
                    Response::ok_empty(),
                    IpcAction::SendText {
                        window: target_window,
                        bytes,
                    },
                ),
                None => (
                    Response::err(format!("unknown key: {key}")),
                    IpcAction::None,
                ),
            }
        }
        "get-cursor" => {
            let (col, row) = terminal.grid.cursor_pos();
            (
                Response::ok(serde_json::json!({ "row": row, "col": col })),
                IpcAction::None,
            )
        }
        "get-size" => (
            Response::ok(serde_json::json!({
                "cols": terminal.cols,
                "rows": terminal.rows,
            })),
            IpcAction::None,
        ),
        "set-title" => {
            let title = req
                .args
                .as_object()
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("handterm")
                .to_string();
            (
                Response::ok_empty(),
                IpcAction::SetTitle {
                    window: target_window,
                    title,
                },
            )
        }
        "close" => (
            Response::ok_empty(),
            IpcAction::Close {
                window: target_window,
            },
        ),
        "ls" => (
            Response::ok(serde_json::json!({
                "commands": [
                    "get-text", "send-text", "send-key",
                    "get-cursor", "get-size", "set-title",
                    "close", "ls"
                ]
            })),
            IpcAction::None,
        ),
        _ => (
            Response::err(format!("unknown command: {}", req.cmd)),
            IpcAction::None,
        ),
    }
}

pub fn spawn_pty_watcher<E: Clone + Send + 'static>(
    thread_name: &str,
    pty_fd: i32,
    ipc_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    let thread_name = thread_name.to_string();
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || fd_watcher_thread(pty_fd, ipc_fd, proxy, event, stop))
        .expect("failed to spawn pty watcher thread");
}

fn fd_watcher_thread<E: Clone + Send + 'static>(
    primary_fd: i32,
    secondary_fd: i32,
    proxy: EventLoopProxy<E>,
    event: E,
    stop: Arc<AtomicBool>,
) {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::BorrowedFd;

    let mut fds = Vec::with_capacity(2);
    fds.push(PollFd::new(
        unsafe { BorrowedFd::borrow_raw(primary_fd) },
        PollFlags::POLLIN | PollFlags::POLLHUP,
    ));
    if secondary_fd >= 0 {
        fds.push(PollFd::new(
            unsafe { BorrowedFd::borrow_raw(secondary_fd) },
            PollFlags::POLLIN,
        ));
    }

    while !stop.load(Ordering::Relaxed) {
        match poll(&mut fds, PollTimeout::from(100u16)) {
            Ok(0) => continue,
            Ok(_) => {
                let _ = proxy.send_event(event.clone());
                if fds[0]
                    .revents()
                    .is_some_and(|revents| revents.contains(PollFlags::POLLHUP))
                {
                    break;
                }
            }
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
}
