use std::convert::TryFrom;
use std::env::{self, set_current_dir};
use std::ffi::{CStr, CString};
use std::fs::{create_dir_all, read_to_string, File, OpenOptions};
use std::io::{IoSlice, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;

use clap::{Parser, Subcommand, ValueEnum};
use nix::libc::{ioctl, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO, TIOCSCTTY};
use nix::mount::{mount, MsFlags};
use nix::sched::{unshare, CloneFlags};
use serde::{Deserialize, Serialize};

use nix::pty::{openpty, OpenptyResult};
use nix::sys::socket::{
    accept, bind, connect, listen, recv, send, sendmsg, socket, socketpair, AddressFamily, Backlog,
    ControlMessage, MsgFlags, SockFlag, SockType, UnixAddr,
};

use nix::unistd::{close, dup2, execvp, fork, pivot_root, setsid, ForkResult};

use anyhow::{Context, Error, Result};

const HAKO_ROOT: &str = "/run/hako";
const EXEC_SOCK: &str = "exec.sock";

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
    path: PathBuf,
    #[serde(default)]
    readonly: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Process {
    #[serde(default)]
    terminal: bool,
    user: User,
    cwd: PathBuf,
    env: Option<Vec<String>>,
    args: Vec<String>,
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

impl Linux {
    fn clone_flags(&self) -> CloneFlags {
        self.namespaces
            .iter()
            .map(|n| match n.r#type.as_str() {
                "pid" => CloneFlags::CLONE_NEWPID,
                "network" => CloneFlags::CLONE_NEWNET,
                "mount" => CloneFlags::CLONE_NEWNS,
                "ipc" => CloneFlags::CLONE_NEWIPC,
                "uts" => CloneFlags::CLONE_NEWUTS,
                "user" => CloneFlags::CLONE_NEWUSER,
                "cgroup" => CloneFlags::CLONE_NEWCGROUP,
                _ => CloneFlags::empty(),
            })
            .fold(CloneFlags::empty(), |a, b| a | b)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Namespace {
    r#type: String,
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
    #[arg(long = "log", default_value = "/dev/stderr")]
    log: PathBuf,
    #[arg(long = "log-format", default_value = "json")]
    log_format: LogFormat,
    #[arg(long = "systemd-cgroup")]
    systemd_cgroup: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Json,
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
        console_socket: Option<PathBuf>,
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
    Delete {
        container_id: String,
        #[arg(long = "force")]
        force: bool,
    },
}

#[derive(Clone)]
struct CreateContext {
    container_id: String,
    path_to_bundle: PathBuf,
    spec: Spec,
    console_socket: Option<PathBuf>,
    pid_file: Option<PathBuf>,
    root: PathBuf,
    log: PathBuf,
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
            let init_pid = child_channel.recv()?;
            println!("from child channel: {}", init_pid);

            // wait until the init process is ready
            println!("from grandchild channel: {}", grandchild_channel.recv()?);

            // update pid file
            if let Some(pid_file) = ctx.pid_file {
                let mut pid_file = OpenOptions::new().create(true).write(true).open(pid_file)?;
                write!(pid_file, "{}", init_pid)?;
            }

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

    if let Some(linux) = &ctx.spec.linux {
        unshare(linux.clone_flags() & CloneFlags::CLONE_NEWPID)?;
    }

    match unsafe { fork().context("failed to create a init process")? } {
        ForkResult::Parent { child: child_pid } => {
            drop(grandchild_channel);

            child_channel.send(child_pid.to_string().as_str())?;

            Ok(())
        }
        ForkResult::Child => {
            drop(child_channel);

            if let Err(err) = init_process(ctx.clone(), grandchild_channel) {
                exit(1);
            }

            exit(0);
        }
    }
}

fn init_process(ctx: CreateContext, grandchild_channel: IpcChannel) -> Result<()> {
    setsid()?;

    if ctx.spec.process.terminal {
        let OpenptyResult { master, slave } = openpty(None, None)?;
        let master = std::mem::ManuallyDrop::new(master);
        let slave = std::mem::ManuallyDrop::new(slave);

        let console_socket = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )?;
        let console_sock_addr = UnixAddr::new(ctx.console_socket.unwrap().as_path())?;
        connect(console_socket.as_raw_fd(), &console_sock_addr)?;
        let iov = [IoSlice::new(b"/dev/ptmx")];
        let fds = [master.as_raw_fd()];
        let cmsg = ControlMessage::ScmRights(&fds);
        sendmsg::<()>(
            console_socket.as_raw_fd(),
            &iov,
            &[cmsg],
            MsgFlags::empty(),
            None,
        )?;

        if unsafe { ioctl(slave.as_raw_fd(), TIOCSCTTY) } < 0 {
            return Err(Error::msg("ioctl error"));
        };

        dup2(slave.as_raw_fd(), STDIN_FILENO)?;
        dup2(slave.as_raw_fd(), STDOUT_FILENO)?;
        dup2(slave.as_raw_fd(), STDERR_FILENO)?;
    }

    if let Some(linux) = &ctx.spec.linux {
        unshare(linux.clone_flags() & !CloneFlags::CLONE_NEWPID)?;
    }

    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    )?;

    mount(
        Some(ctx.spec.root.path.as_path()),
        ctx.spec.root.path.as_path(),
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )?;

    let container_root = PathBuf::from_str(HAKO_ROOT)?.join(
        ctx.container_id
            .chars()
            .take(10)
            .collect::<String>()
            .as_str(),
    );
    let socket_path = container_root.join(EXEC_SOCK);

    create_dir_all(container_root)?;

    let socket = socket(
        AddressFamily::Unix,
        SockType::SeqPacket,
        SockFlag::SOCK_CLOEXEC,
        None,
    )?;
    let sock_addr = UnixAddr::new(socket_path.as_path())?;
    bind(socket.as_raw_fd(), &sock_addr)?;
    listen(&socket, Backlog::new(1)?)?;

    pivot_root(ctx.spec.root.path.as_path(), ctx.spec.root.path.as_path())?;

    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::empty(),
        None::<&str>,
    )?;

    grandchild_channel.send("ready")?;

    // wait start
    accept(socket.as_raw_fd())?;
    set_current_dir(ctx.spec.process.cwd.as_path())?;
    let args: Vec<CString> = ctx
        .spec
        .process
        .args
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap())
        .collect();

    execvp(&args[0], &args)?;

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
                log: cli.log,
            })
            .context("failed to create a container")?;
        }
        Commands::Start { container_id } => {
            println!("start command {}", container_id);
            let container_root = PathBuf::from_str(HAKO_ROOT)?
                .join(container_id.chars().take(10).collect::<String>().as_str());
            let socket_path = container_root.join(EXEC_SOCK);
            let socket = socket(
                AddressFamily::Unix,
                SockType::SeqPacket,
                SockFlag::SOCK_CLOEXEC,
                None,
            )?;
            let sock_addr = UnixAddr::new(&socket_path)?;
            connect(socket.as_raw_fd(), &sock_addr)?;
        }
        Commands::Kill {
            container_id,
            signal,
        } => {
            println!("kill command {} {:?}", container_id, signal);
        }
        Commands::Delete {
            container_id,
            force,
        } => {
            println!("delete command {}", container_id);
        }
    };
    Ok(())
}
