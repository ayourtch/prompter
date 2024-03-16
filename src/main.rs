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
use std::sync::atomic::Ordering;

use termios::*;

fn cursor_goto(my_stdout: &File, x: u16, y: u16) {
    let out = format!("\x1b[{};{}H", y, x);
    write(my_stdout.as_fd(), out.as_bytes());
}

fn main() {
    use nix::libc;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let winch = Arc::new(AtomicBool::new(false));
    let sigquit = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(libc::SIGWINCH, Arc::clone(&winch));
    let _ = signal_hook::flag::register(libc::SIGQUIT, Arc::clone(&sigquit));

    env_logger::init();
    let my_stdin = unsafe { File::from_raw_fd(0) };
    let my_stdout = unsafe { File::from_raw_fd(1) };
    write(
        my_stdout.as_fd(),
        format!("{esc}[2J{esc}[1;1H", esc = 27 as char).as_bytes(),
    );
    let termsize::Size { mut rows, mut cols } = termsize::get().unwrap();

    let mut process = PtyProcess::spawn(Command::new("/bin/bash")).unwrap();
    let pty = process.get_raw_handle().unwrap();

    process.set_window_size(cols, rows - 1);

    let pfd1 = PollFd::new(pty.as_fd(), PollFlags::POLLIN);
    // let stdin = std::io::stdin();
    // let stdout = std::io::stdout();

    let pfd2 = PollFd::new(my_stdin.as_fd(), PollFlags::POLLIN);
    let mut buf = [0u8; 1024];

    let mut parser = vt100::Parser::new(rows - 1, cols, 0);
    let mut curr_screen = parser.screen().clone();

    // crossterm::terminal::enable_raw_mode();
    let mut logfile = File::create("log.txt").unwrap();

    let saved_termios = Termios::from_fd(1).unwrap();
    tcsetattr(pty.as_raw_fd(), TCSANOW, &saved_termios).unwrap();
    let mut termios = Termios::from_fd(pty.as_raw_fd()).unwrap();
    // println!("termios: {:?}", &termios);

    let mut count = 0;
    let mut winch_count = 0;
    let mut last_input: String = format!("");
    let mut status_invoke = 0;
    let mut status_delta_chars = 0;
    let mut status_delta_mark_chars = 0;

    loop {
        let delta = parser.screen().contents_diff(&curr_screen);
        // write(my_stdout.as_fd(), format!("{esc}[2J{esc}[1;1H", esc = 27 as char).as_bytes());
        write(my_stdout.as_fd(), &delta);
        status_delta_chars = delta.len();
        if delta.len() > 0 {
            status_delta_mark_chars = delta.len();
        }

        curr_screen = parser.screen().clone();
        if winch.load(Ordering::Relaxed) {
            winch_count += 1;
            let new_sz = termsize::get().unwrap();
            rows = new_sz.rows;
            cols = new_sz.cols;
            parser.screen_mut().set_size(rows - 1, cols);
            curr_screen.set_size(rows - 1, cols);
            process.set_window_size(cols, rows - 1);
            process.signal(ptyprocess::Signal::SIGWINCH);
            winch.store(false, Ordering::Relaxed);

            let mut parser = vt100::Parser::new(rows - 1, cols, 0);
            curr_screen = parser.screen().clone();
            write(
                my_stdout.as_fd(),
                format!("{esc}[2J{esc}[1;1H", esc = 27 as char).as_bytes(),
            );
        }
        {
            // update status line
            count += 1;
            let (curr_row, curr_col) = curr_screen.cursor_position();
            let mut col = curr_col;
            let mut col_start = curr_col;
            let mut col_end = curr_col;
            loop {
                if let Some(cell) = curr_screen.cell(curr_row, col) {
                    if cell.contents().contains(' ') {
                        break;
                    } else {
                        col_start = col;
                        if col == 0 {
                            break;
                        }
                        col -= 1;
                    }
                } else {
                    break;
                }
            }
            col = curr_col;
            loop {
                if let Some(cell) = curr_screen.cell(curr_row, col) {
                    if cell.contents().contains(' ') {
                        break;
                    } else {
                        col_end = col;
                        col += 1;
                    }
                } else {
                    break;
                }
            }
            let contents = curr_screen.contents_between(curr_row, col_start, curr_row, col_end + 1);
            let attrs = "\x1b[30;47m";
            let status_text = format!(
                "Prompter ({},{}, {}): {} loops, delta ({}, {}) winch: {:?} count {}, contents: '{}', last input: {:?} ({:?})\x1b[K",
                rows, cols, status_invoke,
                count,
                status_delta_chars, status_delta_mark_chars,
                winch,
                winch_count,
                &contents,
                &last_input,
                &last_input.as_bytes()
            );
            // let status_text = format!("{:?}", &termios);
            let status = format!("{}{}", attrs, status_text);

            cursor_goto(&my_stdout, 1, rows);
            let mut last_status_char = if status.len() > cols as usize {
                cols as usize
            } else {
                status.len()
            };
            while !status.is_char_boundary(last_status_char) {
                last_status_char -= 1;
            }

            write(my_stdout.as_fd(), &status[0..last_status_char].as_bytes());
            let attrs = curr_screen.attributes_formatted();
            write(my_stdout.as_fd(), &attrs);
            cursor_goto(&my_stdout, curr_col + 1, curr_row + 1);
        }

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

        let res = poll(&mut fds, PollTimeout::from(100u16));
        if res.is_err() {
            continue;
        }

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
                    // A little hack which does not work if e.g. one does ssh, then another ssh from there.
                    if termios.c_lflag == 2611 {
                        last_input = format!("***SECRET***");
                    } else {
                        last_input = String::from_utf8_lossy(&buf[0..count]).to_string();
                    }
                    if &last_input == "\x07" {
                        status_invoke += 1;
                    } else {
                        write(pty.as_fd(), &buf[0..count]);
                        write(logfile.as_fd(), &buf[0..count]);
                    }
                }
            }
        }
    }
    // crossterm::terminal::disable_raw_mode();

    tcsetattr(1, TCSANOW, &saved_termios).unwrap();

    println!("prompter finished!");
}
