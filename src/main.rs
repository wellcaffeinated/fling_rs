mod cli;
mod client;
mod config;
mod protocol;
mod server;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

fn parse_socket_path(s: &str) -> String {
    s.strip_prefix("unix:").unwrap_or(s).to_owned()
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().collect();

    // Implicit client mode: if the first argument isn't "server", prepend "client"
    // so clap always sees an explicit subcommand.
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
