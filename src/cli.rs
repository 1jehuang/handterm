use crate::backend::Backend;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "handterm")]
#[command(
    author,
    version,
    about = "Cross-platform terminal (Wayland and macOS/Metal) focused on speed"
)]
#[command(
    after_help = "handterm uses a single-process host architecture: repeated launches reuse\nthe running host and open another window in the same process."
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[arg(long, global = true, value_enum)]
    pub backend: Option<Backend>,

    #[arg(long, global = true)]
    pub standalone: bool,

    /// Run a shell command in the initial window instead of an interactive login shell
    #[arg(long = "exec", global = true)]
    pub startup_command: Option<String>,

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
    /// Render a LaTeX math expression in Handterm
    Latex {
        /// LaTeX math body, without `$` delimiters
        expression: String,
    },
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
