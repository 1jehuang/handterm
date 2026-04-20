use crate::config::AppConfig;
pub use crate::daemon_stack::runtime::{ServerDaemon, default_server_socket_path};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn run_server_only(socket: Option<PathBuf>, config: &AppConfig) -> Result<()> {
    crate::daemon_stack::runtime::run_server_only_with_build_id(
        socket,
        config,
        crate::build_info::protocol_build_id(),
    )
}

pub fn ensure_server_running(socket_path: &Path, config_override: Option<&Path>) -> Result<()> {
    let protocol_build_id = crate::build_info::protocol_build_id();
    crate::daemon_stack::runtime::ensure_server_running_with_build_id(
        socket_path,
        config_override,
        &protocol_build_id,
    )
}
