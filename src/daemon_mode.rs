pub fn print_daemon_mode_deprecation(mode: &str) {
    eprintln!(
        "handterm: warning: {mode} uses the deprecated daemon/thin-client path.\nhandterm: the recommended local architecture is the default single-process host path."
    );
}

#[cfg(feature = "cli")]
use crate::backend::Backend;
#[cfg(all(feature = "cli", feature = "cpu"))]
use crate::backend::background_opacity_warning;
#[cfg(feature = "cli")]
use crate::config::AppConfig;
#[cfg(feature = "cli")]
use anyhow::Result;
#[cfg(feature = "cli")]
use std::path::PathBuf;

#[cfg(feature = "cli")]
pub fn run_server_only_command(socket: Option<PathBuf>, config: &AppConfig) -> Result<()> {
    print_daemon_mode_deprecation("`server-only`");
    crate::daemon::run_server_only(socket, config)
}

#[cfg(feature = "cli")]
pub fn run_client_only_command(
    backend: Backend,
    socket: Option<PathBuf>,
    config: AppConfig,
) -> Result<()> {
    print_daemon_mode_deprecation("`client-only`");
    let socket_path = socket.unwrap_or_else(crate::daemon::default_server_socket_path);

    match backend {
        Backend::Cpu => {
            #[cfg(feature = "cpu")]
            {
                if let Some(warning) =
                    background_opacity_warning(backend, config.style.background_opacity)
                {
                    eprintln!("{warning}");
                }
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
