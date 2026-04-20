#[cfg_attr(feature = "gpu", allow(dead_code))]
#[cfg(all(feature = "cpu", feature = "standalone"))]
pub mod app;
pub mod backend;
pub mod build_info;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "daemon-client")]
#[allow(dead_code)]
pub mod client;
pub mod color;
pub mod config;
#[cfg(feature = "daemon-server")]
pub mod daemon;
pub mod daemon_mode;
pub mod fd_watcher;
#[allow(dead_code)]
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
#[cfg(any(feature = "standalone", feature = "daemon-server"))]
pub mod pty;
pub mod remote;
#[cfg(all(feature = "cpu", feature = "daemon-client"))]
pub mod remote_app;
#[cfg(all(feature = "gpu", feature = "daemon-client"))]
pub mod remote_gpu_app;
pub mod render;
pub mod runtime;
#[cfg(feature = "daemon-server")]
#[allow(dead_code)]
pub mod server;
#[cfg(feature = "standalone")]
pub mod standalone_support;
pub mod visual;
#[cfg(feature = "standalone")]
pub mod workloads;

pub use daemon_mode::print_daemon_mode_deprecation;
pub use handterm_common::grid;
pub use handterm_common::parser;
pub use handterm_common::protocol;
pub use handterm_common::server_sync;
pub use handterm_common::terminal;
#[cfg(feature = "cli")]
pub use runtime::{run_cli, run_with_cli, should_reuse_existing_host};
