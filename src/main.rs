use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Config {
    oci_version: String,
    root: Root,
    process: Process,
    hostname: Option<String>,
    domainname: Option<String>,
    mounts: Option<Vec<Mount>>,
    linux: Option<Linux>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Root {
    path: String,
    #[serde(default)]
    readonly: bool,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Process {
    #[serde(default)]
    terminal: bool,
    console_size: Option<ConsoleSize>,
    user: User,
    cwd: String,
    env: Option<Vec<String>>,
    args: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ConsoleSize {
    height: usize,
    width: usize,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct User {
    uid: usize,
    gid: usize,
    umask: Option<usize>,
    additional_gids: Option<Vec<usize>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Mount {
    destination: String,
    source: Option<String>,
    options: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Linux {
    namespaces: Vec<Namespace>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Namespace {
    r#type: String,
    path: Option<String>,
}

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
