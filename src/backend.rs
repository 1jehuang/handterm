use anyhow::{Result, bail};
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    Cpu,
    Gpu,
}

pub fn resolve_backend(requested: Option<Backend>) -> Result<Backend> {
    let default = if cfg!(feature = "gpu") {
        Backend::Gpu
    } else if cfg!(feature = "cpu") {
        Backend::Cpu
    } else {
        bail!("handterm was built without a CPU or GPU backend");
    };

    let selected = requested.unwrap_or(default);

    match selected {
        Backend::Cpu if cfg!(feature = "cpu") => Ok(Backend::Cpu),
        Backend::Gpu if cfg!(feature = "gpu") => Ok(Backend::Gpu),
        Backend::Cpu => bail!("CPU backend is not compiled into this build"),
        Backend::Gpu => bail!("GPU backend is not compiled into this build"),
    }
}

#[cfg(feature = "cpu")]
pub fn background_opacity_warning(
    selected: Backend,
    background_opacity: f64,
) -> Option<&'static str> {
    (selected == Backend::Cpu && background_opacity < 1.0).then_some(
        "handterm: background_opacity requires the GPU backend on Wayland; CPU rendering via softbuffer is opaque because its Wayland buffer format is Xrgb8888",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backend_prefers_gpu_when_available() {
        let backend = resolve_backend(None).expect("default backend should resolve");
        if cfg!(feature = "gpu") {
            assert_eq!(backend, Backend::Gpu);
        } else {
            assert_eq!(backend, Backend::Cpu);
        }
    }

    #[test]
    fn explicit_cpu_request_requires_cpu_feature() {
        let resolved = resolve_backend(Some(Backend::Cpu));
        if cfg!(feature = "cpu") {
            assert_eq!(resolved.expect("cpu backend should exist"), Backend::Cpu);
        } else {
            assert!(resolved.is_err());
        }
    }

    #[test]
    fn explicit_gpu_request_requires_gpu_feature() {
        let resolved = resolve_backend(Some(Backend::Gpu));
        if cfg!(feature = "gpu") {
            assert_eq!(resolved.expect("gpu backend should exist"), Backend::Gpu);
        } else {
            assert!(resolved.is_err());
        }
    }

    #[cfg(feature = "cpu")]
    #[test]
    fn warns_when_transparency_is_requested_on_cpu_backend() {
        assert!(background_opacity_warning(Backend::Cpu, 0.9).is_some());
        assert!(background_opacity_warning(Backend::Cpu, 1.0).is_none());
        assert!(background_opacity_warning(Backend::Gpu, 0.9).is_none());
    }
}
