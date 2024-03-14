use std::ffi::CString;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::process::Command;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::unistd::read;
use nix::unistd::write;

use ptyprocess::PtyProcess;
use std::fs::File;
use std::os::fd::FromRawFd;

use termios::*;

fn main() {
    env_logger::init();
    let my_stdin = unsafe { File::from_raw_fd(0) };
    let my_stdout = unsafe { File::from_raw_fd(1) };
    write(
        my_stdout.as_fd(),
        format!("{esc}[2J{esc}[1;1H", esc = 27 as char).as_bytes(),
    );
    let termsize::Size { rows, cols } = termsize::get().unwrap();

    let mut process = PtyProcess::spawn(Command::new("/bin/bash")).unwrap();
    let pty = process.get_raw_handle().unwrap();

    process.set_window_size(cols, rows-1);

    let pfd1 = PollFd::new(pty.as_fd(), PollFlags::POLLIN);
    // let stdin = std::io::stdin();
    // let stdout = std::io::stdout();

    let pfd2 = PollFd::new(my_stdin.as_fd(), PollFlags::POLLIN);
    let mut buf = [0u8; 1024];

    let mut parser = vt100::Parser::new(rows-1, cols, 0);
    let mut curr_screen = parser.screen().clone();

    // crossterm::terminal::enable_raw_mode();
    let mut logfile = File::create("log.txt").unwrap();

    let saved_termios = Termios::from_fd(1).unwrap();
    tcsetattr(pty.as_raw_fd(), TCSANOW, &saved_termios).unwrap();
    let mut termios = Termios::from_fd(pty.as_raw_fd()).unwrap();
    // println!("termios: {:?}", &termios);

    loop {
        let delta = parser.screen().contents_diff(&curr_screen);
        // write(my_stdout.as_fd(), format!("{esc}[2J{esc}[1;1H", esc = 27 as char).as_bytes());
        write(my_stdout.as_fd(), &delta);
        curr_screen = parser.screen().clone();

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
        let mut new_termios = Termios::from_fd(pty.as_raw_fd()).unwrap();
        if new_termios != termios {
            tcsetattr(1, TCSANOW, &new_termios).unwrap();
            // write(logfile.as_fd(), "AYXX: termios changed!\n".as_bytes());
            termios = new_termios;
        }
        // PTY reading
        if let Some(revents) = fds[0].revents() {
            if revents == PollFlags::POLLIN {
                let count = read(fds[0].as_fd().as_raw_fd(), &mut buf[..]);
                if let Ok(count) = count {
                    // write(my_stdout.as_fd(), &buf[0..count]);
                    write(logfile.as_fd(), &buf[0..count]);
                    parser.process(&buf[0..count]);
                }
            }
        }
        // STDIN
        if let Some(revents) = fds[1].revents() {
            if revents == PollFlags::POLLIN {
                let count = read(fds[1].as_fd().as_raw_fd(), &mut buf[..]);
                if let Ok(count) = count {
                    write(pty.as_fd(), &buf[0..count]);
                    write(logfile.as_fd(), &buf[0..count]);
                }
            }
        }
    }
    // crossterm::terminal::disable_raw_mode();

    tcsetattr(1, TCSANOW, &saved_termios).unwrap();

    println!("prompter finished!");
}
