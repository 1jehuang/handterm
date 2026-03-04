use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "handterm")]
#[command(author, version, about = "Wayland-native terminal focused on speed")]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

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
}
