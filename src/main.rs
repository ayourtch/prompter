use std::ffi::CString;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::process::Command;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::unistd::read;
use nix::unistd::write;

use ptyprocess::PtyProcess;



fn main() {
    println!("Launching prompter");
    let mut process = PtyProcess::spawn(Command::new("/bin/bash")).unwrap();
    let pty = process.get_raw_handle().unwrap();

    let pfd1 = PollFd::new(pty.as_fd(), PollFlags::POLLIN);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    let pfd2 = PollFd::new(stdin.as_fd(), PollFlags::POLLIN);
    let mut buf = [0u8; 1024];

    loop {
        match process.is_alive() {
            Ok(alive) => {
                if !alive {
                    println!("Child exited.");
                    break;
                }
            }
            Err(errno) => {
                println!("Child exited with errno {}", errno);
                break;
            }
        }
        let mut fds = [pfd1, pfd2];
        let res = poll(&mut fds, PollTimeout::from(100u16)).unwrap();
        if let Some(revents) = fds[0].revents() {
            if revents == PollFlags::POLLIN {
                let count = read(fds[0].as_fd().as_raw_fd(), &mut buf[..]);
                if let Ok(count) = count {
                    write(stdout.as_fd(), &buf[0..count]);
                }
            }
        }
        if let Some(revents) = fds[1].revents() {
            if revents == PollFlags::POLLIN {
                let count = read(fds[1].as_fd().as_raw_fd(), &mut buf[..]);
                if let Ok(count) = count {
                    write(pty.as_fd(), &buf[0..count]);
                    write(stdout.as_fd(), &buf[0..count]);
                }
            }
        }
    }

    let mut parser = vt100::Parser::new(24, 80, 0);

    let screen = parser.screen().clone();
    parser.process(b"this text is \x1b[31mRED\x1b[m");
    assert_eq!(
        parser.screen().cell(0, 13).unwrap().fgcolor(),
        vt100::Color::Idx(1),
    );

    let screen = parser.screen().clone();
    parser.process(b"\x1b[3D\x1b[32mGREEN");
    assert_eq!(
        parser.screen().contents_formatted(),
        &b"\x1b[?25h\x1b[m\x1b[H\x1b[Jthis text is \x1b[32mGREEN"[..],
    );
    assert_eq!(
        parser.screen().contents_diff(&screen),
        &b"\x1b[1;14H\x1b[32mGREEN"[..],
    );
    println!("Hello, world!");
}
