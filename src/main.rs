mod app;
mod cli;
mod color;
mod config;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::AppConfig;

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
        None => app::run(config),
    }
}
