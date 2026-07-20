use crate::ipc::{IpcAction, Request, Response};
use crate::terminal::Terminal;

pub fn handle_ipc_request(terminal: &mut Terminal, req: &Request) -> (Response, IpcAction) {
    let target_window = req
        .args
        .as_object()
        .and_then(|o| o.get("window_id"))
        .and_then(|v| v.as_u64());

    match req.cmd.as_str() {
        "get-text" => {
            let text = if let Some(obj) = req.args.as_object() {
                let start = obj.get("start_row").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
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
        "apply-scroll-delta" => {
            let delta_rows = req
                .args
                .as_object()
                .and_then(|o| o.get("delta_rows"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let steps = delta_rows.abs().ceil() as usize;
            let max = terminal.grid.scrollback_len();
            if delta_rows > 0.0 {
                terminal.grid.scroll_offset = (terminal.grid.scroll_offset + steps).min(max);
            } else {
                terminal.grid.scroll_offset = terminal.grid.scroll_offset.saturating_sub(steps);
            }
            (
                Response::ok(serde_json::json!({
                    "backend": "terminal",
                    "scroll_offset": terminal.grid.scroll_offset,
                    "scrollback_len": terminal.grid.scrollback_len(),
                    "smooth_supported": false,
                })),
                IpcAction::None,
            )
        }
        "get-scroll-state" => (
            Response::ok(serde_json::json!({
                "backend": "terminal",
                "scroll_offset": terminal.grid.scroll_offset,
                "scrollback_len": terminal.grid.scrollback_len(),
                "rows": terminal.grid.rows,
                "smooth_supported": false,
            })),
            IpcAction::None,
        ),
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
                    "send-key-event", "send-ime-commit",
                    "get-cursor", "apply-scroll-delta", "get-scroll-state", "get-size", "set-title",
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
