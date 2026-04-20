use anyhow::Result;
use clap::Parser;
use handterm::config::AppConfig;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "handterm-server")]
#[command(about = "Deprecated server-only reference binary for daemon mode")]
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
    handterm::print_daemon_mode_deprecation("`handterm-server`");
    let args = Args::parse();
    let config = AppConfig::load(args.config.as_deref())?;
    handterm::daemon::run_server_only(args.socket, &config)
}
