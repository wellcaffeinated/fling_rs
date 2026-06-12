mod cli;
mod client;
mod config;
mod protocol;
mod sandbox;
mod server;

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

/// The canonical program name. When the binary is invoked under any other name
/// (e.g. via a symlink), that name is treated as the command to relay.
const PROGRAM_NAME: &str = "fling";

/// Default socket used by symlink-mode invocations, overridable via the
/// `FLING_SOCKET` environment variable.
const DEFAULT_SOCKET: &str = "unix:/run/fling.sock";

fn parse_socket_path(s: &str) -> String {
    s.strip_prefix("unix:").unwrap_or(s).to_owned()
}

/// The basename the binary was invoked as (argv[0] with directories stripped).
fn invoked_as(argv0: &str) -> &str {
    Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(PROGRAM_NAME)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let program = invoked_as(&args[0]).to_string();

    // Symlink mode: when invoked under any name other than `fling`, that name
    // *is* the command to relay. Every remaining argument is forwarded verbatim
    // to the remote command, so the socket can only come from the environment.
    if program != PROGRAM_NAME {
        let socket = std::env::var("FLING_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string());
        let path = parse_socket_path(&socket);
        let cmd_args = args[1..].to_vec();
        let code = client::run(&path, &program, &cmd_args).await.unwrap_or_else(|e| {
            eprintln!("fling: {e}");
            1
        });
        std::process::exit(code);
    }

    // Implicit client mode: if the first argument isn't "server", prepend
    // "client" so clap always sees an explicit subcommand.
    let mut args = args;
    if args.len() > 1 && args[1] != "server" {
        args.insert(1, "client".to_string());
    }

    let cli = Cli::parse_from(args);

    match cli.command {
        Commands::Server { socket, config: config_path } => {
            let config = config::load(&config_path)?;
            let path = parse_socket_path(&socket);
            server::run(&path, config).await?;
        }
        Commands::Client { socket, cmd, args } => {
            let path = parse_socket_path(&socket);
            let code = client::run(&path, &cmd, &args).await.unwrap_or_else(|e| {
                eprintln!("fling: {e}");
                1
            });
            std::process::exit(code);
        }
    }

    Ok(())
}
