#[cfg_attr(feature = "gpu", allow(dead_code))]
#[cfg(all(feature = "cpu", feature = "standalone"))]
pub mod app;
pub mod backend;
pub mod build_info;
#[cfg(feature = "daemon-client")]
#[allow(dead_code)]
pub mod client;
#[cfg(feature = "cli")]
pub mod cli;
pub mod color;
pub mod config;
#[cfg(feature = "daemon-server")]
pub mod daemon;
#[allow(dead_code)]
pub mod font;
pub mod frontend;
#[cfg(feature = "gpu")]
pub mod gpu_frame;
#[cfg(all(feature = "gpu", feature = "standalone"))]
pub mod gpu_app;
#[cfg(feature = "gpu")]
pub mod gpu_runtime;
#[cfg(feature = "standalone")]
pub mod ipc;
pub mod input;
#[cfg(feature = "standalone")]
pub mod metrics;
#[cfg(any(feature = "standalone", feature = "daemon-server"))]
pub mod pty;
pub mod remote;
#[cfg(all(feature = "cpu", feature = "daemon-client"))]
pub mod remote_app;
#[cfg(all(feature = "gpu", feature = "daemon-client"))]
pub mod remote_gpu_app;
pub mod render;
#[cfg(feature = "daemon-server")]
#[allow(dead_code)]
pub mod server;
#[cfg(feature = "standalone")]
pub mod standalone_support;
pub mod visual;
#[cfg(feature = "standalone")]
pub mod workloads;

pub use handterm_common::grid;
pub use handterm_common::parser;
pub use handterm_common::protocol;
pub use handterm_common::terminal;

#[cfg(feature = "cli")]
use anyhow::Result;
#[cfg(feature = "cli")]
use backend::{Backend, resolve_backend};
#[cfg(all(feature = "cpu", feature = "cli"))]
use backend::background_opacity_warning;
#[cfg(feature = "cli")]
use clap::Parser;
#[cfg(feature = "cli")]
use cli::{Cli, Command};
#[cfg(feature = "cli")]
use config::AppConfig;
#[cfg(feature = "standalone")]
use metrics::{format_bench_results, run_quick_bench};

#[cfg(all(feature = "cpu", feature = "cli"))]
fn open_window_in_existing_host(
    to: Option<std::path::PathBuf>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<()> {
    let socket_path = match to {
        Some(path) => path,
        None => ipc::find_socket().ok_or_else(|| anyhow::anyhow!("no running handterm host found"))?,
    };

    let mut args = serde_json::Map::new();
    if let Some(cols) = cols {
        args.insert("cols".into(), serde_json::json!(cols));
    }
    if let Some(rows) = rows {
        args.insert("rows".into(), serde_json::json!(rows));
    }

    let req = ipc::Request {
        cmd: "open-window".to_string(),
        args: serde_json::Value::Object(args),
    };
    let resp = ipc::send_command(&socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(resp.error.unwrap_or_else(|| "open-window request failed".to_string()));
    }
    Ok(())
}

#[cfg(feature = "cli")]
pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    run_with_cli(cli)
}

#[cfg(feature = "cli")]
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
        Some(Command::OpenWindow { to, cols, rows }) => {
            #[cfg(feature = "cpu")]
            {
                open_window_in_existing_host(to, cols, rows)
            }
            #[cfg(not(feature = "cpu"))]
            {
                let _ = (to, cols, rows);
                unreachable!("open-window requires the CPU host frontend")
            }
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
                        if let Some(socket_path) = ipc::find_socket()
                            && ipc::send_command(
                                &socket_path,
                                &ipc::Request {
                                    cmd: "open-window".to_string(),
                                    args: serde_json::Value::Object(serde_json::Map::new()),
                                },
                            )
                            .is_ok()
                        {
                            return Ok(());
                        }
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
