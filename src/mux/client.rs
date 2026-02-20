use anyhow::{Context, Result};
use std::io::Read;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::protocol::{self, ClientMsg, ServerMsg};
use super::terminal::{
    self, scrollback_line_count, InputAction, InputState, RawModeGuard, ScrollState,
};

enum ClientEvent {
    ServerMsg(ServerMsg),
    InputBytes(Vec<u8>),
    ServerDisconnected,
}

pub fn run(session_name: &str, socket_path: &Path) -> Result<i32> {
    // Open /dev/tty
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("Cannot open /dev/tty for mux client")?;
    let tty_fd = tty.as_raw_fd();

    let (term_cols, term_rows) = terminal::get_term_size(tty_fd)?;

    // Verify termios
    {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut termios) } != 0 {
            anyhow::bail!("Not a terminal device");
        }
    }

    let inner_rows = term_rows.saturating_sub(1);
    if inner_rows == 0 || term_cols == 0 {
        anyhow::bail!("Terminal too small");
    }

    // Connect to server
    let sock = UnixStream::connect(socket_path).context("Failed to connect to mux server")?;
    let mut sock_writer = sock.try_clone().context("Failed to clone socket")?;

    // Set a write timeout so the client event loop doesn't block
    // indefinitely if the server is slow to read.
    let _ = sock_writer.set_write_timeout(Some(Duration::from_secs(5)));

    // Install panic hook
    terminal::install_panic_hook();

    // Enter raw mode (also enables mouse tracking for scroll wheel)
    let _guard = RawModeGuard::activate(&mut tty)?;

    // Send initial Resize to server
    protocol::write_client_msg(
        &mut sock_writer,
        &ClientMsg::Resize {
            cols: term_cols,
            rows: inner_rows,
        },
    )?;

    // Wait for Resized from server to know the PTY dimensions.
    // Use a timeout so we don't block forever in raw mode if the server hangs.
    let mut sock_reader = sock
        .try_clone()
        .context("Failed to clone socket for reader")?;
    sock_reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .context("Failed to set handshake read timeout")?;
    let (pty_cols, pty_rows) = match protocol::read_server_msg(&mut sock_reader) {
        Ok(ServerMsg::Resized { cols, rows }) => (cols, rows),
        Ok(ServerMsg::Exited(code)) => return Ok(code),
        Ok(_) => (term_cols, inner_rows),
        Err(_) => anyhow::bail!("Timed out waiting for server handshake"),
    };

    // Create local parser with server's PTY dimensions
    let mut parser = vt100::Parser::new(pty_rows, pty_cols, super::SCROLLBACK_LINES);

    // Process the screen dump that follows
    match protocol::read_server_msg(&mut sock_reader) {
        Ok(ServerMsg::Output(data)) => {
            parser.process(&data);
        }
        Ok(ServerMsg::Exited(code)) => return Ok(code),
        Ok(_) => {}
        Err(_) => return Ok(1),
    }

    // Clear timeout for normal operation (reader thread handles its own blocking)
    sock_reader.set_read_timeout(None)?;

    // Create ratatui terminal
    let mut terminal = terminal::create_terminal(tty_fd, term_cols, term_rows)?;

    // Channel for events
    let (tx, rx) = mpsc::channel::<ClientEvent>();

    // Socket reader thread
    let tx_sock = tx.clone();
    std::thread::spawn(move || loop {
        match protocol::read_server_msg(&mut sock_reader) {
            Ok(msg) => {
                if tx_sock.send(ClientEvent::ServerMsg(msg)).is_err() {
                    break;
                }
            }
            Err(_) => {
                let _ = tx_sock.send(ClientEvent::ServerDisconnected);
                break;
            }
        }
    });

    // Input reader thread
    let tx_input = tx.clone();
    let tty_input_fd = unsafe { libc::dup(tty_fd) };
    if tty_input_fd < 0 {
        anyhow::bail!("Failed to dup tty fd for input");
    }
    std::thread::spawn(move || {
        let mut tty_input = unsafe { std::fs::File::from_raw_fd(tty_input_fd) };
        let mut buf = [0u8; 4096];
        loop {
            match tty_input.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx_input
                        .send(ClientEvent::InputBytes(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    let project_name = super::project_name_for_session(session_name);
    let mut input_state = InputState::new();
    let mut dirty = true;
    let mut mouse_tracking_on = false;

    let mut last_cols = term_cols;
    let mut last_rows = term_rows;
    let mut current_inner_rows = inner_rows;

    loop {
        let timeout = if dirty {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(50)
        };
        let event = rx.recv_timeout(timeout);
        match event {
            Ok(ClientEvent::ServerMsg(msg)) => match msg {
                ServerMsg::Output(data) => {
                    parser.process(&data);
                    dirty = true;
                }
                ServerMsg::Resized { cols, rows } => {
                    parser.set_size(rows, cols);
                    parser.process(b"\x1b[H\x1b[2J");
                    input_state.scroll_offset = 0;
                    dirty = true;
                }
                ServerMsg::Exited(code) => {
                    return Ok(code);
                }
            },
            Ok(ClientEvent::InputBytes(data)) => {
                let max_scrollback = scrollback_line_count(&mut parser);
                let actions =
                    input_state.process(&data, current_inner_rows, last_cols, max_scrollback);
                for action in actions {
                    match action {
                        InputAction::Forward(bytes) => {
                            let _ = protocol::write_client_msg(
                                &mut sock_writer,
                                &ClientMsg::Input(bytes),
                            );
                        }
                        InputAction::Detach => {
                            return Ok(0);
                        }
                        InputAction::Kill => {
                            let _ = protocol::write_client_msg(&mut sock_writer, &ClientMsg::Kill);
                        }
                        InputAction::Redraw => {
                            dirty = true;
                        }
                    }
                }
            }
            Ok(ClientEvent::ServerDisconnected) => {
                return Ok(0);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Flush any buffered incomplete escape sequence
                let max_scrollback = scrollback_line_count(&mut parser);
                let pending_actions =
                    input_state.flush_pending(current_inner_rows, last_cols, max_scrollback);
                for action in pending_actions {
                    match action {
                        InputAction::Forward(bytes) => {
                            let _ = protocol::write_client_msg(
                                &mut sock_writer,
                                &ClientMsg::Input(bytes),
                            );
                        }
                        InputAction::Redraw => {
                            dirty = true;
                        }
                        _ => {}
                    }
                }

                // Check for terminal resize
                if let Ok((cols, rows)) = terminal::get_term_size(tty_fd) {
                    if cols != last_cols || rows != last_rows {
                        last_cols = cols;
                        last_rows = rows;
                        let new_inner = rows.saturating_sub(1);
                        if new_inner > 0 && cols > 0 {
                            current_inner_rows = new_inner;
                            let _ = protocol::write_client_msg(
                                &mut sock_writer,
                                &ClientMsg::Resize {
                                    cols,
                                    rows: new_inner,
                                },
                            );
                            terminal = terminal::create_terminal(tty_fd, cols, rows)?;
                            // Clear stale content left by the terminal emulator's
                            // resize reflow.  Without this, ratatui's diff skips
                            // "empty" cells that still show old content on screen.
                            terminal.clear()?;
                        }
                        input_state.scroll_offset = 0;
                        dirty = true;
                    }
                }

                if dirty {
                    let max_scrollback = scrollback_line_count(&mut parser);

                    // Enable mouse tracking only when there's scrollback content
                    let want_mouse = max_scrollback > 0;
                    if want_mouse != mouse_tracking_on {
                        mouse_tracking_on = want_mouse;
                        terminal::set_mouse_tracking(tty_fd, mouse_tracking_on);
                    }

                    parser.set_scrollback(input_state.scroll_offset);
                    let session_name = session_name.to_string();
                    let project_name = project_name.clone();
                    let screen = parser.screen();
                    let scroll = ScrollState {
                        offset: input_state.scroll_offset,
                        max: max_scrollback,
                    };
                    terminal
                        .draw(|f| {
                            terminal::draw_frame(f, screen, &session_name, &project_name, &scroll);
                        })
                        .context("Failed to draw terminal frame")?;
                    parser.set_scrollback(0);
                    dirty = false;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(0)
}
