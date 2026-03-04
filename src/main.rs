mod app;
mod cli;
mod color;
mod config;
mod font;
mod grid;
mod ipc;
mod metrics;
mod parser;
mod pty;
mod terminal;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};
use config::AppConfig;
use metrics::{run_quick_bench, format_bench_results};

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
            println!("{}", format_bench_results(&out));
            Ok(())
        }
        Some(Command::Remote { to, cmd, args }) => {
            let socket_path = match to {
                Some(path) => path,
                None => ipc::find_socket()
                    .ok_or_else(|| anyhow::anyhow!("no running handterm instance found"))?,
            };
            let parsed_args: serde_json::Value = serde_json::from_str(&args)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let req = ipc::Request {
                cmd,
                args: parsed_args,
            };
            let resp = ipc::send_command(&socket_path, &req)?;
            let output = serde_json::to_string_pretty(&resp)?;
            println!("{output}");
            if !resp.ok {
                std::process::exit(1);
            }
            Ok(())
        }
        None => app::run(config),
    }
}
