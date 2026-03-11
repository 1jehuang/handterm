use anyhow::Result;
use clap::Parser;
use handterm::config::AppConfig;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "handterm-server")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = AppConfig::load(args.config.as_deref())?;
    handterm::daemon::run_server_only(args.socket, &config)
}
