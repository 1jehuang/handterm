use anyhow::Result;
use clap::Parser;
use handterm::config::AppConfig;
use std::path::PathBuf;

fn default_server_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join("handterm-server.sock")
}

#[derive(Debug, Parser)]
#[command(name = "handterm-client-gpu")]
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
    handterm::remote_gpu_app::run(config, socket_path)
}
