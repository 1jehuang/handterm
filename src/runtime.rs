#[cfg(all(feature = "cpu", feature = "cli"))]
use crate::backend::background_opacity_warning;
#[cfg(feature = "cli")]
use crate::backend::{Backend, resolve_backend};
#[cfg(feature = "cli")]
use crate::cli::{Cli, Command};
#[cfg(feature = "cli")]
use crate::config::AppConfig;
#[cfg(feature = "standalone")]
use crate::metrics::{format_bench_results, run_quick_bench};
#[cfg(feature = "cli")]
use anyhow::Result;
#[cfg(feature = "cli")]
use clap::Parser;

#[cfg(feature = "cli")]
fn open_window_in_existing_host(
    backend: Backend,
    to: Option<std::path::PathBuf>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<()> {
    let socket_path = match to {
        Some(path) => path,
        None => crate::ipc::find_socket_for_backend(backend).ok_or_else(|| {
            anyhow::anyhow!(
                "no running handterm {} host found",
                match backend {
                    Backend::Cpu => "CPU",
                    Backend::Gpu => "GPU",
                }
            )
        })?,
    };

    let mut args = serde_json::Map::new();
    if let Some(cols) = cols {
        args.insert("cols".into(), serde_json::json!(cols));
    }
    if let Some(rows) = rows {
        args.insert("rows".into(), serde_json::json!(rows));
    }

    let req = crate::ipc::Request {
        cmd: "open-window".to_string(),
        args: serde_json::Value::Object(args),
    };
    let resp = crate::ipc::send_command(&socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(
            resp.error
                .unwrap_or_else(|| "open-window request failed".to_string())
        );
    }
    Ok(())
}

#[cfg(feature = "cli")]
pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    run_with_cli(cli)
}

pub fn print_daemon_mode_deprecation(mode: &str) {
    eprintln!(
        "handterm: warning: {mode} uses the deprecated daemon/thin-client path.\nhandterm: the recommended local architecture is the default single-process host path."
    );
}

#[cfg(feature = "cli")]
pub fn should_reuse_existing_host(cli: &Cli) -> bool {
    !cli.standalone
}

#[cfg(feature = "cli")]
pub fn run_with_cli(cli: Cli) -> Result<()> {
    let backend = resolve_backend(cli.backend)?;
    let config = AppConfig::load(cli.config.as_deref())?;

    #[cfg(feature = "cpu")]
    let warn_if_cpu_opacity = || {
        if let Some(warning) = background_opacity_warning(backend, config.style.background_opacity)
        {
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
            open_window_in_existing_host(backend, to, cols, rows)
        }
        Some(Command::ServerOnly { socket }) => {
            print_daemon_mode_deprecation("`server-only`");
            crate::daemon::run_server_only(socket, &config)
        }
        Some(Command::ClientOnly { socket }) => {
            print_daemon_mode_deprecation("`client-only`");
            let socket_path = socket.unwrap_or_else(crate::daemon::default_server_socket_path);
            match backend {
                Backend::Cpu => {
                    #[cfg(feature = "cpu")]
                    {
                        warn_if_cpu_opacity();
                        crate::remote_app::run(config, socket_path)
                    }
                    #[cfg(not(feature = "cpu"))]
                    {
                        unreachable!("client-only mode requires the CPU frontend");
                    }
                }
                Backend::Gpu => {
                    #[cfg(feature = "gpu")]
                    {
                        crate::remote_gpu_app::run(config, socket_path)
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
                None => crate::ipc::find_socket_for_backend(backend)
                    .ok_or_else(|| anyhow::anyhow!("no running handterm instance found"))?,
            };
            let parsed_args: serde_json::Value = serde_json::from_str(&args)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let req = crate::ipc::Request {
                cmd,
                args: parsed_args,
            };
            let resp = crate::ipc::send_command(&socket_path, &req)?;
            let output = serde_json::to_string_pretty(&resp)?;
            println!("{output}");
            if !resp.ok {
                std::process::exit(1);
            }
            Ok(())
        }
        None => match backend {
            Backend::Cpu => {
                #[cfg(feature = "cpu")]
                {
                    warn_if_cpu_opacity();
                    if should_reuse_existing_host(&cli)
                        && let Some(socket_path) = crate::ipc::find_socket_for_backend(Backend::Cpu)
                        && crate::ipc::send_command(
                            &socket_path,
                            &crate::ipc::Request {
                                cmd: "open-window".to_string(),
                                args: serde_json::Value::Object(serde_json::Map::new()),
                            },
                        )
                        .is_ok()
                    {
                        return Ok(());
                    }
                    crate::app::run(config)
                }
                #[cfg(not(feature = "cpu"))]
                {
                    unreachable!("backend resolution allowed unavailable CPU backend");
                }
            }
            Backend::Gpu => {
                #[cfg(feature = "gpu")]
                {
                    if should_reuse_existing_host(&cli)
                        && let Some(socket_path) = crate::ipc::find_socket_for_backend(Backend::Gpu)
                        && crate::ipc::send_command(
                            &socket_path,
                            &crate::ipc::Request {
                                cmd: "open-window".to_string(),
                                args: serde_json::Value::Object(serde_json::Map::new()),
                            },
                        )
                        .is_ok()
                    {
                        return Ok(());
                    }
                    crate::gpu_app::run(config)
                }
                #[cfg(not(feature = "gpu"))]
                {
                    unreachable!("backend resolution allowed unavailable GPU backend");
                }
            }
        },
    }
}
