use std::env;
use std::ffi::CString;

use nix::libc::SIGCHLD;
use nix::sched::{clone, CloneFlags};
use nix::sys::wait::waitpid;
use nix::unistd::execv;
use nix::unistd::getpid;

fn exec() -> isize {
    let args: Vec<CString> = env::args().map(|s| CString::new(s).unwrap()).collect();
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
            CloneFlags::empty(),
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
