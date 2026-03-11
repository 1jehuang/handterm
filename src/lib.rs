#[cfg_attr(feature = "gpu", allow(dead_code))]
#[cfg(feature = "cpu")]
pub mod app;
pub mod backend;
pub mod build_info;
#[allow(dead_code)]
pub mod client;
pub mod cli;
pub mod color;
pub mod config;
pub mod daemon;
#[allow(dead_code)]
pub mod font;
pub mod frontend;
#[cfg(feature = "gpu")]
pub mod gpu_frame;
#[cfg(feature = "gpu")]
pub mod gpu_app;
#[cfg(feature = "gpu")]
pub mod gpu_runtime;
pub mod ipc;
pub mod input;
pub mod metrics;
pub mod pty;
pub mod remote;
#[cfg(feature = "cpu")]
pub mod remote_app;
#[cfg(feature = "gpu")]
pub mod remote_gpu_app;
pub mod render;
#[allow(dead_code)]
pub mod server;
pub mod visual;
pub mod workloads;

pub use handterm_common::grid;
pub use handterm_common::parser;
pub use handterm_common::protocol;
pub use handterm_common::terminal;

use anyhow::Result;
use backend::{Backend, resolve_backend};
#[cfg(feature = "cpu")]
use backend::background_opacity_warning;
use clap::Parser;
use cli::{Cli, Command};
use config::AppConfig;
use metrics::{format_bench_results, run_quick_bench};

pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    run_with_cli(cli)
}

pub fn run_with_cli(cli: Cli) -> Result<()> {
    let backend = resolve_backend(cli.backend)?;
    let config = AppConfig::load(cli.config.as_deref())?;

    #[cfg(feature = "cpu")]
    let warn_if_cpu_opacity = || {
        if let Some(warning) = background_opacity_warning(backend, config.style.background_opacity) {
            eprintln!("{warning}");
        }
    };

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
        Some(Command::ServerOnly { socket }) => daemon::run_server_only(socket, &config),
        Some(Command::ClientOnly { socket }) => {
            let socket_path = socket.unwrap_or_else(daemon::default_server_socket_path);
            match backend {
                Backend::Cpu => {
                    #[cfg(feature = "cpu")]
                    {
                        warn_if_cpu_opacity();
                        remote_app::run(config, socket_path)
                    }
                    #[cfg(not(feature = "cpu"))]
                    {
                        unreachable!("client-only mode requires the CPU frontend");
                    }
                }
                Backend::Gpu => {
                    #[cfg(feature = "gpu")]
                    {
                        remote_gpu_app::run(config, socket_path)
                    }
                    #[cfg(not(feature = "gpu"))]
                    {
                        unreachable!("client-only mode requires the GPU frontend");
                    }
                }
            }
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
        None => {
            if !cli.standalone {
                let socket_path = daemon::default_server_socket_path();
                daemon::ensure_server_running(&socket_path, cli.config.as_deref())?;
                match backend {
                    Backend::Cpu => {
                        #[cfg(feature = "cpu")]
                        {
                            warn_if_cpu_opacity();
                            return remote_app::run(config, socket_path);
                        }
                        #[cfg(not(feature = "cpu"))]
                        {
                            unreachable!("daemon client mode requires the CPU frontend");
                        }
                    }
                    Backend::Gpu => {
                        #[cfg(feature = "gpu")]
                        {
                            return remote_gpu_app::run(config, socket_path);
                        }
                        #[cfg(not(feature = "gpu"))]
                        {
                            unreachable!("daemon client mode requires the GPU frontend");
                        }
                    }
                }
            }

            match backend {
                Backend::Cpu => {
                    #[cfg(feature = "cpu")]
                    {
                        warn_if_cpu_opacity();
                        app::run(config)
                    }
                    #[cfg(not(feature = "cpu"))]
                    {
                        unreachable!("backend resolution allowed unavailable CPU backend");
                    }
                }
                Backend::Gpu => {
                    #[cfg(feature = "gpu")]
                    {
                        gpu_app::run(config)
                    }
                    #[cfg(not(feature = "gpu"))]
                    {
                        unreachable!("backend resolution allowed unavailable GPU backend");
                    }
                }
            }
        }
    }
}
