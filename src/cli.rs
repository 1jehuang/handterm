use crate::backend::Backend;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "handterm")]
#[command(author, version, about = "Wayland-native terminal focused on speed")]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[arg(long, global = true, value_enum)]
    pub backend: Option<Backend>,

    #[arg(long, global = true)]
    pub standalone: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the active style and runtime defaults
    PrintConfig,
    /// Generate the default config file if missing
    InitConfig,
    /// Run quick local performance benchmarks
    Bench,
    /// Ask a running CPU host to open another window
    OpenWindow {
        /// Socket path (auto-detected if omitted)
        #[arg(long)]
        to: Option<PathBuf>,

        /// Override columns for the new window
        #[arg(long)]
        cols: Option<u16>,

        /// Override rows for the new window
        #[arg(long)]
        rows: Option<u16>,
    },
    /// Run only the daemon/server process
    ServerOnly {
        /// Socket path to bind (default: $XDG_RUNTIME_DIR/handterm-server.sock)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Run a window frontend connected to a running handterm server
    ClientOnly {
        /// Socket path to connect (default: $XDG_RUNTIME_DIR/handterm-server.sock)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Send a command to a running handterm instance
    #[command(name = "@")]
    Remote {
        /// Socket path (auto-detected if omitted)
        #[arg(long)]
        to: Option<PathBuf>,

        /// Command to send
        cmd: String,

        /// JSON arguments (e.g. '{"text":"hello"}')
        #[arg(default_value = "{}")]
        args: String,
    },
}
