use std::path::PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fling", about = "Unix socket command relay")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run in server mode, listening on a Unix socket
    Server {
        /// Socket path: unix:/run/fling.sock or /run/fling.sock
        #[arg(long, short, default_value = "unix:/run/fling.sock")]
        socket: String,
        /// Config file listing allowed commands
        #[arg(long, short, default_value = "/etc/fling/config.toml")]
        config: PathBuf,
    },
    /// Relay a command through a Unix socket (default mode)
    Client {
        /// Socket path: unix:/run/fling.sock or /run/fling.sock
        #[arg(long, short)]
        socket: String,
        /// Command name (must be in the server's allowlist)
        cmd: String,
        /// Arguments forwarded verbatim to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}
