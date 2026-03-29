use crate::frontend::KeyEventKind;
use crate::ipc::{IpcAction, Request, Response, SyntheticKeyEvent};

#[derive(Debug)]
pub enum HostControlRequest {
    OpenWindow {
        cols: Option<u16>,
        rows: Option<u16>,
    },
    FocusWindow {
        window_id: u64,
    },
    ListWindows,
    SyntheticKeyEvent {
        window: Option<u64>,
        event: SyntheticKeyEvent,
    },
    SyntheticImeCommit {
        window: Option<u64>,
        text: String,
    },
}

pub fn parse_host_control_request(req: &Request) -> Result<Option<HostControlRequest>, Response> {
    let parsed = match req.cmd.as_str() {
        "open-window" => {
            let (cols, rows) = requested_window_size(req);
            Some(HostControlRequest::OpenWindow { cols, rows })
        }
        "focus-window" => {
            let Some(window_id) = target_window_from_args(req) else {
                return Err(Response::err("missing 'window_id' argument"));
            };
            Some(HostControlRequest::FocusWindow { window_id })
        }
        "list-windows" => Some(HostControlRequest::ListWindows),
        "send-key-event" => Some(HostControlRequest::SyntheticKeyEvent {
            window: target_window_from_args(req),
            event: synthetic_key_event_from_request(req).map_err(Response::err)?,
        }),
        "send-ime-commit" => {
            let text =
                ime_commit_text(req).ok_or_else(|| Response::err("missing 'text' argument"))?;
            Some(HostControlRequest::SyntheticImeCommit {
                window: target_window_from_args(req),
                text: text.to_string(),
            })
        }
        _ => None,
    };

    Ok(parsed)
}

pub fn target_window_from_args(req: &Request) -> Option<u64> {
    req.args
        .as_object()
        .and_then(|o| o.get("window_id"))
        .and_then(|v| v.as_u64())
}

pub fn host_list_windows_response(window_ids: Vec<u64>, focused_window: Option<u64>) -> Response {
    Response::ok(serde_json::json!({
        "windows": window_ids,
        "count": window_ids.len(),
        "focused": focused_window,
    }))
}

pub fn host_ls_response(include_host_commands: bool) -> Response {
    let mut commands = vec![
        "get-text",
        "send-text",
        "send-key",
        "send-key-event",
        "send-ime-commit",
        "get-cursor",
        "get-size",
        "set-title",
        "close",
        "ls",
    ];
    if include_host_commands {
        commands.extend(["open-window", "focus-window", "list-windows"]);
    }
    Response::ok(serde_json::json!({ "commands": commands }))
}

pub fn into_ipc_action(request: HostControlRequest) -> (Response, IpcAction) {
    match request {
        HostControlRequest::OpenWindow { cols, rows } => {
            (Response::ok_empty(), IpcAction::OpenWindow { cols, rows })
        }
        HostControlRequest::FocusWindow { window_id } => {
            (Response::ok_empty(), IpcAction::FocusWindow(window_id))
        }
        HostControlRequest::ListWindows => (Response::ok_empty(), IpcAction::None),
        HostControlRequest::SyntheticKeyEvent { window, event } => (
            Response::ok_empty(),
            IpcAction::SyntheticKeyEvent { window, event },
        ),
        HostControlRequest::SyntheticImeCommit { window, text } => (
            Response::ok_empty(),
            IpcAction::SyntheticImeCommit { window, text },
        ),
    }
}

fn requested_window_size(req: &Request) -> (Option<u16>, Option<u16>) {
    let cols = req
        .args
        .as_object()
        .and_then(|o| o.get("cols"))
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok());
    let rows = req
        .args
        .as_object()
        .and_then(|o| o.get("rows"))
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok());
    (cols, rows)
}

fn ime_commit_text(req: &Request) -> Option<&str> {
    req.args
        .as_object()
        .and_then(|o| o.get("text"))
        .and_then(|v| v.as_str())
        .filter(|text| !text.is_empty())
}

fn synthetic_key_event_from_request(req: &Request) -> Result<SyntheticKeyEvent, &'static str> {
    let Some(args) = req.args.as_object() else {
        return Err("missing JSON object arguments");
    };

    let kind = match args.get("kind").and_then(|v| v.as_str()).unwrap_or("press") {
        "press" => KeyEventKind::Press,
        "repeat" => KeyEventKind::Repeat,
        "release" => KeyEventKind::Release,
        _ => return Err("invalid 'kind' argument"),
    };
    let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
        return Err("missing 'key' argument");
    };

    Ok(SyntheticKeyEvent {
        kind,
        key: key.to_string(),
        text: args
            .get("text")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        ctrl: args.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false),
        alt: args.get("alt").and_then(|v| v.as_bool()).unwrap_or(false),
        shift: args.get("shift").and_then(|v| v.as_bool()).unwrap_or(false),
        super_key: args
            .get("super")
            .or_else(|| args.get("super_key"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        hyper: args.get("hyper").and_then(|v| v.as_bool()).unwrap_or(false),
        meta: args.get("meta").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(cmd: &str, args: serde_json::Value) -> Request {
        Request {
            cmd: cmd.to_string(),
            args,
        }
    }

    #[test]
    fn parses_open_window_sizes() {
        let parsed = parse_host_control_request(&req(
            "open-window",
            serde_json::json!({ "cols": 90, "rows": 30 }),
        ))
        .expect("open-window should parse")
        .expect("open-window should produce control request");

        match parsed {
            HostControlRequest::OpenWindow { cols, rows } => {
                assert_eq!(cols, Some(90));
                assert_eq!(rows, Some(30));
            }
            other => panic!("expected open-window request, got {other:?}"),
        }
    }

    #[test]
    fn focus_window_requires_window_id() {
        let response = parse_host_control_request(&req("focus-window", serde_json::json!({})))
            .expect_err("missing window_id should error");
        assert_eq!(
            response.error.as_deref(),
            Some("missing 'window_id' argument")
        );
    }

    #[test]
    fn parses_synthetic_key_event() {
        let parsed = parse_host_control_request(&req(
            "send-key-event",
            serde_json::json!({
                "window_id": 7,
                "kind": "repeat",
                "key": "A",
                "text": "a",
                "ctrl": true,
            }),
        ))
        .expect("synthetic key event should parse")
        .expect("synthetic key event should produce control request");

        match parsed {
            HostControlRequest::SyntheticKeyEvent { window, event } => {
                assert_eq!(window, Some(7));
                assert_eq!(event.kind, KeyEventKind::Repeat);
                assert_eq!(event.key, "A");
                assert_eq!(event.text.as_deref(), Some("a"));
                assert!(event.ctrl);
                assert!(!event.alt);
            }
            other => panic!("expected synthetic key request, got {other:?}"),
        }
    }

    #[test]
    fn host_ls_lists_host_specific_commands_when_requested() {
        let response = host_ls_response(true);
        let commands = response
            .data
            .as_ref()
            .and_then(|data| data.get("commands"))
            .and_then(|value| value.as_array())
            .expect("commands array should exist");

        assert!(
            commands
                .iter()
                .any(|value| value.as_str() == Some("open-window"))
        );
        assert!(
            commands
                .iter()
                .any(|value| value.as_str() == Some("focus-window"))
        );
        assert!(
            commands
                .iter()
                .any(|value| value.as_str() == Some("list-windows"))
        );
    }
}
