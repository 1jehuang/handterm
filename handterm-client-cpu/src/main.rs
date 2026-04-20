use anyhow::Result;
use clap::Parser;
use handterm::backend::{Backend, background_opacity_warning};
use handterm::config::AppConfig;
use std::path::PathBuf;

fn default_server_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join("handterm-server.sock")
}

#[derive(Debug, Parser)]
#[command(name = "handterm-client-cpu")]
#[command(about = "Deprecated CPU thin client for the daemon/reference path")]
#[command(
    after_help = "This binary uses the deprecated daemon/thin-client path and is best treated as a reference or experimental architecture.\nRecommended local path: use the default single-process host via `handterm`."
)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = AppConfig::load(args.config.as_deref())?;
    let socket_path = args.socket.unwrap_or_else(default_server_socket_path);
    if let Some(warning) = background_opacity_warning(Backend::Cpu, config.style.background_opacity)
    {
        eprintln!("{warning}");
    }
    handterm::remote_app::run(config, socket_path)
}
