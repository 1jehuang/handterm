// `app::run`'s only caller is the CLI runtime (runtime.rs), so without `cli`
// the module compiles but is unreachable.
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
#[cfg(all(feature = "cpu", feature = "standalone"))]
pub mod app;
pub mod backend;
pub mod build_info;
#[cfg(feature = "cli")]
pub mod cli;
pub mod color;
pub mod config;
pub mod fd_watcher;
// `font` exposes a wide glyph/shaping API surface. With `local-fonts` off, the
// font-discovery-dependent parts are compiled but unreachable, so allow
// dead_code only for that feature combination.
#[cfg_attr(not(feature = "local-fonts"), allow(dead_code))]
pub mod font;
pub mod frontend;
#[cfg(all(feature = "gpu", feature = "standalone"))]
pub mod gpu_app;
#[cfg(feature = "gpu")]
pub mod gpu_frame;
#[cfg(feature = "gpu")]
pub mod gpu_runtime;
#[cfg(feature = "standalone")]
pub mod host_commands;
#[cfg(feature = "standalone")]
pub mod host_input;
pub mod input;
#[cfg(feature = "standalone")]
pub mod ipc;
#[cfg(feature = "standalone")]
pub mod metrics;
#[cfg(feature = "standalone")]
pub mod native_scroll;
pub mod platform;
pub mod profiling;
#[cfg(feature = "standalone")]
pub mod pty;
pub mod render;
pub mod runtime;
#[cfg(feature = "standalone")]
pub mod standalone_support;
pub mod visual;
#[cfg(feature = "standalone")]
pub mod workloads;

pub use handterm_common::grid;
pub use handterm_common::parser;
pub use handterm_common::protocol;
pub use handterm_common::server_sync;
pub use handterm_common::terminal;
#[cfg(feature = "cli")]
pub use runtime::{run_cli, run_with_cli, should_reuse_existing_host};
