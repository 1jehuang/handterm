use crate::backend::Backend;
use std::path::Path;

pub fn protocol_build_id() -> String {
    format!(
        "{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        env!("HANDTERM_GIT_COMMIT"),
        compiled_features()
    )
}

pub fn startup_banner(backend: Backend, socket_path: Option<&Path>) -> String {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    let socket = socket_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<none>".to_string());

    format!(
        "handterm {} ({}) backend={} profile={} features={} pid={} exe={} socket={}",
        env!("CARGO_PKG_VERSION"),
        env!("HANDTERM_GIT_COMMIT"),
        backend_name(backend),
        std::env::var("PROFILE").unwrap_or_else(|_| profile_name().to_string()),
        compiled_features(),
        std::process::id(),
        exe,
        socket,
    )
}

fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Cpu => "cpu",
        Backend::Gpu => "gpu",
    }
}

fn profile_name() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn compiled_features() -> &'static str {
    #[cfg(all(feature = "cpu", feature = "gpu", feature = "ligatures"))]
    const FEATURES: &str = "cpu,gpu,ligatures";
    #[cfg(all(feature = "cpu", feature = "gpu", not(feature = "ligatures")))]
    const FEATURES: &str = "cpu,gpu";
    #[cfg(all(feature = "cpu", not(feature = "gpu"), feature = "ligatures"))]
    const FEATURES: &str = "cpu,ligatures";
    #[cfg(all(feature = "cpu", not(feature = "gpu"), not(feature = "ligatures")))]
    const FEATURES: &str = "cpu";
    #[cfg(all(not(feature = "cpu"), feature = "gpu", feature = "ligatures"))]
    const FEATURES: &str = "gpu,ligatures";
    #[cfg(all(not(feature = "cpu"), feature = "gpu", not(feature = "ligatures")))]
    const FEATURES: &str = "gpu";
    #[cfg(all(not(feature = "cpu"), not(feature = "gpu"), feature = "ligatures"))]
    const FEATURES: &str = "ligatures";
    #[cfg(all(not(feature = "cpu"), not(feature = "gpu"), not(feature = "ligatures")))]
    const FEATURES: &str = "none";

    FEATURES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_banner_includes_core_build_fields() {
        let banner = startup_banner(Backend::Cpu, Some(Path::new("/tmp/handterm-cpu.sock")));
        assert!(banner.contains("handterm "));
        assert!(banner.contains("backend=cpu"));
        assert!(banner.contains("features="));
        assert!(banner.contains("socket=/tmp/handterm-cpu.sock"));
    }

    #[test]
    fn protocol_build_id_includes_version_and_commit() {
        let build_id = protocol_build_id();
        assert!(build_id.contains(env!("CARGO_PKG_VERSION")));
        assert!(build_id.contains(env!("HANDTERM_GIT_COMMIT")));
    }
}
