use anyhow::Result;
use handterm::backend::{resolve_backend, Backend};
use handterm::config::AppConfig;

fn main() -> Result<()> {
    let config = AppConfig::load(None)?;
    let socket_path = handterm::daemon::default_server_socket_path();
    let backend = resolve_backend(None)?;

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
