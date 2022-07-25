use std::env::set_current_dir;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::os::unix::io::AsRawFd;
use std::path::Path;

// This code was highly inspired from Frank Denis (@jedisct1) 'daemonize-simple' crate,
// available at https://github.com/jedisct1/rust-daemonize-simple/blob/master/src/unix.rs .
// MIT licensed according to https://github.com/jedisct1/rust-daemonize-simple/blob/master/Cargo.toml
pub unsafe fn daemonize(
    chdir: &Path,
    pid_file: &Path,
    log_file: &Path,
) -> Result<(), &'static str> {
    match libc::fork() {
        -1 => return Err("fork() failed"),
        0 => {}
        _ => {
            libc::_exit(0);
        }
    }
    libc::setsid();
    match libc::fork() {
        -1 => return Err("Second fork() failed"),
        0 => {}
        _ => {
            libc::_exit(0);
        }
    };

    let fd = OpenOptions::new()
        .read(true)
        .open("/dev/null")
        .map_err(|_| "Unable to open the stdin file")?;
    if libc::dup2(fd.as_raw_fd(), 0) == -1 {
        return Err("dup2(stdin) failed");
    }
    let fd = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .map_err(|_| "Unable to open the stdout file")?;
    if libc::dup2(fd.as_raw_fd(), 1) == -1 {
        return Err("dup2(stdout) failed");
    }
    let fd = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .map_err(|_| "Unable to open the stderr file")?;
    if libc::dup2(fd.as_raw_fd(), 2) == -1 {
        return Err("dup2(stderr) failed");
    }

    let pid = match libc::getpid() {
        -1 => return Err("getpid() failed"),
        pid => pid,
    };
    let pid_str = format!("{}", pid);
    File::create(pid_file)
        .map_err(|_| "Creating the PID file failed")?
        .write_all(pid_str.as_bytes())
        .map_err(|_| "Writing to the PID file failed")?;

    set_current_dir(chdir).map_err(|_| "chdir() failed")?;

    Ok(())
}
