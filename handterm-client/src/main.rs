use anyhow::Result;
use clap::Parser;
use handterm::backend::{resolve_backend, Backend};
use handterm::config::AppConfig;
use std::path::PathBuf;

fn default_server_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/handterm-{}", std::process::id()));
    PathBuf::from(runtime_dir).join("handterm-server.sock")
}

#[derive(Debug, Parser)]
#[command(name = "handterm-client")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long, value_enum)]
    backend: Option<Backend>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = AppConfig::load(args.config.as_deref())?;
    let socket_path = args.socket.unwrap_or_else(default_server_socket_path);
    let backend = resolve_backend(args.backend)?;

    match backend {
        Backend::Cpu => {
            #[cfg(feature = "cpu")]
            {
                if let Some(warning) = handterm::backend::background_opacity_warning(
                    backend,
                    config.style.background_opacity,
                ) {
                    eprintln!("{warning}");
                }
                handterm::remote_app::run(config, socket_path)
            }
            #[cfg(not(feature = "cpu"))]
            {
                unreachable!("client-only binary requires CPU frontend when CPU is selected")
            }
        }
        Backend::Gpu => {
            #[cfg(feature = "gpu")]
            {
                handterm::remote_gpu_app::run(config, socket_path)
            }
            #[cfg(not(feature = "gpu"))]
            {
                unreachable!("client-only binary requires GPU frontend when GPU is selected")
            }
        }
    }
}
