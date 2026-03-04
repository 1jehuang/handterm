mod app;
mod cli;
mod color;
mod config;
mod grid;
mod metrics;
mod pty;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::AppConfig;
use metrics::run_quick_bench;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(cli.config.as_deref())?;

    match cli.command {
        Some(Command::PrintConfig) => {
            let rendered = toml::to_string_pretty(&config)?;
            println!("{rendered}");
            Ok(())
        }
        Some(Command::InitConfig) => {
            let path = AppConfig::save_default_if_missing(cli.config.as_deref())?;
            println!("config initialized at {}", path.display());
            Ok(())
        }
        Some(Command::Bench) => {
            let out = run_quick_bench(config.window.columns, config.window.rows)?;
            println!("pty_spawn_us={}", out.spawn_us);
            println!("shell_ready_us={}", out.shell_ready_us);
            println!("grid_alloc_us={}", out.grid_alloc_us);
            println!("ascii_grid_mb_per_sec={:.2}", out.ascii_grid_mb_per_sec);
            Ok(())
        }
        None => app::run(config),
    }
}
