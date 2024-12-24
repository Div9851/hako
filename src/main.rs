use std::env;
use std::ffi::CString;

use nix::libc::SIGCHLD;
use nix::mount::{mount, MsFlags};
use nix::sched::{clone, CloneFlags};
use nix::sys::wait::waitpid;
use nix::unistd::execv;
use nix::unistd::getpid;
use nix::unistd::pivot_root;

fn exec() -> isize {
    let args: Vec<CString> = env::args().map(|s| CString::new(s).unwrap()).collect();
    let new_root = "/home/waritasa/mountpoint";
    env::set_current_dir(new_root).unwrap();
    println!("current directory: {:?}", env::current_dir().unwrap());
    mount(
        Some(""),
        "/",
        Some(""),
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        Some(""),
    )
    .unwrap();
    mount(
        Some("."),
        ".",
        Some(""),
        MsFlags::MS_BIND | MsFlags::MS_REC,
        Some(""),
    )
    .unwrap();
    pivot_root(".", ".").unwrap();
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::empty(),
        Some(""),
    )
    .unwrap();
    execv(&args[1], &args[1..]).unwrap();
    0
}

fn main() {
    println!("pid: {}", getpid());
    const STACK_SIZE: usize = 1024 * 1024;
    let mut stack: [u8; STACK_SIZE] = [0; STACK_SIZE];
    unsafe {
        let pid = clone(
            Box::new(exec),
            &mut stack,
            CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID,
            Some(SIGCHLD),
        )
        .unwrap();
        println!("child pid: {}", pid);
        match waitpid(pid, None) {
            Ok(status) => println!("child process exited {:?}", status),
            Err(err) => panic!("failed to waitpid {:?}", err),
        }
    }
}
