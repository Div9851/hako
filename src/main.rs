use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version = "0.0.1", about = "Open Container Initiative runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// output the state of a container
    State { container_id: String },
    /// create a container
    Create {
        container_id: String,
        path_to_bundle: PathBuf,
    },
    /// executes the user defined process in a created container
    Start { container_id: String },
    /// kill sends the specified signal (default: SIGTERM) to the container's init process
    Kill {
        container_id: String,
        signal: Option<i32>,
    },
    /// delete any resources held by the container often used with detached container
    Delete { container_id: String },
}

fn main() {
    let cli = Cli::parse();
    match &cli.command {
        Commands::State { container_id } => {
            println!("state command {}", container_id);
        }
        Commands::Create {
            container_id,
            path_to_bundle,
        } => {
            println!("create command {} {:?}", container_id, path_to_bundle);
        }
        Commands::Start { container_id } => {
            println!("start command {}", container_id);
        }
        Commands::Kill {
            container_id,
            signal,
        } => {
            println!("kill command {} {:?}", container_id, signal);
        }
        Commands::Delete { container_id } => {
            println!("delete command {}", container_id);
        }
    }
}
