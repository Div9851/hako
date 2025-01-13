use std::convert::TryFrom;
use std::env;
use std::fs::{create_dir_all, read_to_string, remove_file};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::exit;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use nix::sys::socket::{
    accept, bind, connect, listen, recv, send, socket, socketpair, AddressFamily, Backlog,
    MsgFlags, SockFlag, SockType, UnixAddr,
};
use nix::unistd::{close, fork, ForkResult};

use anyhow::{Context, Error, Result};

const HAKO_ROOT: &str = "/run/hako";

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Spec {
    oci_version: String,
    root: Root,
    process: Process,
    hostname: Option<String>,
    domainname: Option<String>,
    mounts: Option<Vec<Mount>>,
    linux: Option<Linux>,
}

impl TryFrom<&Path> for Spec {
    type Error = Error;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        let spec_json =
            read_to_string(path).with_context(|| format!("failed to read {:?}", path))?;
        let spec: Self = serde_json::from_str(&spec_json)
            .with_context(|| format!("failed to parse {:?}", path))?;
        Ok(spec)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Root {
    path: String,
    #[serde(default)]
    readonly: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct ConsoleSize {
    height: usize,
    width: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct User {
    uid: usize,
    gid: usize,
    umask: Option<usize>,
    additional_gids: Option<Vec<usize>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Mount {
    destination: String,
    source: Option<String>,
    options: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Linux {
    namespaces: Vec<Namespace>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Namespace {
    r#type: String,
    path: Option<String>,
}

struct IpcChannel {
    fd: OwnedFd,
}

impl IpcChannel {
    fn new(fd: OwnedFd) -> Self {
        Self { fd }
    }

    fn send(&self, msg: &str) -> Result<()> {
        send(self.fd.as_raw_fd(), msg.as_bytes(), MsgFlags::empty())?;
        Ok(())
    }

    fn recv(&self) -> Result<String> {
        let mut buf = vec![0; 1024];
        let len = recv(self.fd.as_raw_fd(), &mut buf, MsgFlags::empty())?;
        buf.truncate(len);
        Ok(String::from_utf8(buf)?)
    }
}

#[derive(Parser)]
#[command(version = "0.0.1", about = "Open Container Initiative runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    #[arg(long = "root", default_value = HAKO_ROOT)]
    root: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    /// output the state of a container
    State { container_id: String },
    /// create a container
    Create {
        container_id: String,
        #[arg(short = 'b', long = "bundle")]
        path_to_bundle: Option<PathBuf>,
        #[arg(long = "console-socket")]
        console_socket: Option<usize>,
        #[arg(long = "pid-file")]
        pid_file: Option<PathBuf>,
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

#[derive(Clone)]
struct CreateContext {
    container_id: String,
    path_to_bundle: PathBuf,
    spec: Spec,
    console_socket: Option<usize>,
    pid_file: Option<PathBuf>,
    root: PathBuf,
}

fn create(ctx: CreateContext) -> Result<()> {
    let (parent_child_sock, child_parent_sock) = socketpair(
        AddressFamily::Unix,
        SockType::SeqPacket,
        None,
        SockFlag::SOCK_CLOEXEC,
    )?;

    let (parent_grandchild_sock, grandchild_parent_sock) = socketpair(
        AddressFamily::Unix,
        SockType::SeqPacket,
        None,
        SockFlag::SOCK_CLOEXEC,
    )?;

    match unsafe { fork().context("failed to create a intermediate process")? } {
        ForkResult::Parent { child: _ } => {
            drop(child_parent_sock);
            drop(grandchild_parent_sock);

            let child_channel = IpcChannel::new(parent_child_sock);
            let grandchild_channel = IpcChannel::new(parent_grandchild_sock);

            // wait until the intermediate process is ready
            child_channel.recv()?;

            // wait until the init process is ready
            grandchild_channel.recv()?;

            // TODO: update pid file

            Ok(())
        }
        ForkResult::Child => {
            drop(parent_child_sock);
            drop(parent_grandchild_sock);

            let child_channel = IpcChannel::new(child_parent_sock);
            let grandchild_channel = IpcChannel::new(grandchild_parent_sock);

            if let Err(_) = intermediate_process(ctx.clone(), child_channel, grandchild_channel) {
                exit(1);
            }
            exit(0);
        }
    }
}

fn intermediate_process(
    ctx: CreateContext,
    child_channel: IpcChannel,
    grandchild_channel: IpcChannel,
) -> Result<()> {
    // TODO: set up cgroup
    // TODO: unshare PID namespace

    match unsafe { fork().context("failed to create a init process")? } {
        ForkResult::Parent { child: child_pid } => {
            drop(grandchild_channel);

            child_channel.send(child_pid.to_string().as_str())?;

            Ok(())
        }
        ForkResult::Child => {
            drop(child_channel);

            if let Err(_) = init_process(ctx.clone(), grandchild_channel) {
                exit(1);
            }

            exit(0);
        }
    }
}

fn init_process(ctx: CreateContext, grandchild_channel: IpcChannel) -> Result<()> {
    // TODO: unshare rest of namespaces
    // TODO: pivot_root
    grandchild_channel.send("ready")?;

    // TODO: wait start
    // TODO: exec

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::State { container_id } => {
            println!("state command {}", container_id);
        }
        Commands::Create {
            container_id,
            path_to_bundle,
            console_socket,
            pid_file,
        } => {
            println!("create command {}", container_id);
            let path_to_bundle = path_to_bundle.unwrap_or(env::current_dir()?);
            let spec = Spec::try_from(path_to_bundle.join("config.json").as_path())?;

            create(CreateContext {
                container_id,
                path_to_bundle,
                spec,
                console_socket,
                pid_file,
                root: cli.root,
            })
            .context("failed to create a container")?;
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
    };
    Ok(())
}
