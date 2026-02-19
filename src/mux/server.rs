use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;

use crate::config;
use crate::session;

use super::protocol::{self, ClientMsg, ServerMsg};

enum ServerEvent {
    PtyOutput(Vec<u8>),
    NewClient(UnixStream),
    ClientMsg { id: u64, msg: ClientMsg },
    ClientDisconnected(u64),
    ChildExited,
}

struct ClientEntry {
    writer: UnixStream,
    cols: u16,
    rows: u16,
    has_resized: bool,
}

/// RAII guard that removes socket + PID file on drop (including panics).
struct CleanupGuard {
    session_name: String,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        session::remove_socket(&self.session_name);
        session::remove_pid(&self.session_name);
    }
}

/// Set PTY size via direct ioctl.
fn set_pty_size(pty: &pty_process::blocking::Pty, rows: u16, cols: u16) -> Result<()> {
    let fd = pty.as_raw_fd();
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) };
    if ret == -1 {
        let err = io::Error::last_os_error();
        anyhow::bail!("ioctl TIOCSWINSZ on fd {}: {}", fd, err);
    }
    Ok(())
}

fn write_bytes_to_pty(pty: &pty_process::blocking::Pty, data: &[u8]) -> Result<()> {
    use std::os::unix::io::AsFd;
    let fd = pty.as_fd();
    let raw_fd = fd.as_raw_fd();
    let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
    let result = file.write_all(data);
    std::mem::forget(file);
    result.map_err(Into::into)
}

pub fn run(session_name: &str) -> Result<()> {
    // Load session metadata
    let sess = session::load(session_name)?;
    if sess.command.is_empty() {
        anyhow::bail!("Session '{}' has no command configured.", session_name);
    }

    // Derive workspace path
    let home = config::home_dir()?;
    let workspace = Path::new(&home)
        .join(".box")
        .join("workspaces")
        .join(session_name);

    // Create Unix socket
    let socket_path = session::socket_path(session_name)?;
    // Remove stale socket if it exists
    let _ = std::fs::remove_file(&socket_path);
    let listener =
        UnixListener::bind(&socket_path).context("Failed to bind Unix socket for mux server")?;

    // Write PID file
    session::write_pid(session_name, std::process::id())?;

    // Cleanup guard removes socket + PID on drop
    let _cleanup = CleanupGuard {
        session_name: session_name.to_string(),
    };

    // Open PTY with default size
    let default_cols: u16 = 80;
    let default_rows: u16 = 24;
    let pty = pty_process::blocking::Pty::new().context("Failed to open PTY")?;
    let pts = pty.pts().context("Failed to get PTY slave")?;
    set_pty_size(&pty, default_rows, default_cols)?;

    // Spawn child
    let mut cmd = pty_process::blocking::Command::new(&sess.command[0]);
    cmd.args(&sess.command[1..]);
    if workspace.is_dir() {
        cmd.current_dir(&workspace);
    }
    let mut child = cmd
        .spawn(&pts)
        .with_context(|| format!("Failed to spawn {:?} in PTY", &sess.command))?;
    // Drop pts so the server doesn't hold the slave side open
    drop(pts);

    // Create vt100 parser for screen state
    let mut parser = vt100::Parser::new(default_rows, default_cols, 10_000);

    let mut pty_cols = default_cols;
    let mut pty_rows = default_rows;

    // Channel for events
    let (tx, rx) = mpsc::channel::<ServerEvent>();

    // PTY reader thread
    let pty_read_fd = unsafe { libc::dup(pty.as_raw_fd()) };
    if pty_read_fd < 0 {
        anyhow::bail!("Failed to dup PTY fd");
    }
    let mut pty_reader = unsafe { std::fs::File::from_raw_fd(pty_read_fd) };
    let tx_pty = tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx_pty.send(ServerEvent::ChildExited);
                    break;
                }
                Ok(n) => {
                    let _ = tx_pty.send(ServerEvent::PtyOutput(buf[..n].to_vec()));
                }
            }
        }
    });

    // Socket accept thread
    let tx_accept = tx.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    if tx_accept.send(ServerEvent::NewClient(s)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut clients: HashMap<u64, ClientEntry> = HashMap::new();
    let mut next_client_id: u64 = 0;

    // Main event loop
    loop {
        let event = rx.recv();
        match event {
            Ok(ServerEvent::PtyOutput(data)) => {
                parser.process(&data);
                // Broadcast to all clients
                let msg = ServerMsg::Output(data);
                let mut disconnected = Vec::new();
                for (&id, client) in clients.iter_mut() {
                    if protocol::write_server_msg(&mut client.writer, &msg).is_err() {
                        disconnected.push(id);
                    }
                }
                for id in disconnected {
                    clients.remove(&id);
                }
                // Recalculate size after disconnects
                if !clients.is_empty() {
                    recalc_size(&clients, &pty, &mut parser, &mut pty_cols, &mut pty_rows);
                }
            }
            Ok(ServerEvent::NewClient(stream)) => {
                let id = next_client_id;
                next_client_id += 1;

                // Clone the stream for the reader thread
                let reader_stream = match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                clients.insert(
                    id,
                    ClientEntry {
                        writer: stream,
                        cols: 0,
                        rows: 0,
                        has_resized: false,
                    },
                );

                // Spawn per-client reader thread
                let tx_client = tx.clone();
                std::thread::spawn(move || {
                    let mut r = reader_stream;
                    loop {
                        match protocol::read_client_msg(&mut r) {
                            Ok(msg) => {
                                if tx_client.send(ServerEvent::ClientMsg { id, msg }).is_err() {
                                    break;
                                }
                            }
                            Err(_) => {
                                let _ = tx_client.send(ServerEvent::ClientDisconnected(id));
                                break;
                            }
                        }
                    }
                });
            }
            Ok(ServerEvent::ClientMsg { id, msg }) => match msg {
                ClientMsg::Resize { cols, rows } => {
                    if let Some(client) = clients.get_mut(&id) {
                        let first_resize = !client.has_resized;
                        client.cols = cols;
                        client.rows = rows;
                        client.has_resized = true;

                        if first_resize {
                            // Send current PTY size
                            let _ = protocol::write_server_msg(
                                &mut client.writer,
                                &ServerMsg::Resized {
                                    cols: pty_cols,
                                    rows: pty_rows,
                                },
                            );
                            // Send screen dump
                            let contents = parser.screen().contents_formatted();
                            if !contents.is_empty() {
                                let _ = protocol::write_server_msg(
                                    &mut client.writer,
                                    &ServerMsg::Output(contents),
                                );
                            }
                        }

                        // Recalculate effective size
                        recalc_size_and_broadcast(
                            &mut clients,
                            &pty,
                            &mut parser,
                            &mut pty_cols,
                            &mut pty_rows,
                        );
                    }
                }
                ClientMsg::Input(data) => {
                    let _ = write_bytes_to_pty(&pty, &data);
                }
                ClientMsg::Kill => {
                    let _ = child.kill();
                }
            },
            Ok(ServerEvent::ClientDisconnected(id)) => {
                clients.remove(&id);
                if !clients.is_empty() {
                    recalc_size_and_broadcast(
                        &mut clients,
                        &pty,
                        &mut parser,
                        &mut pty_cols,
                        &mut pty_rows,
                    );
                }
                // Server keeps running with zero clients
            }
            Ok(ServerEvent::ChildExited) => {
                // Drain remaining PTY output
                while let Ok(ServerEvent::PtyOutput(data)) = rx.try_recv() {
                    parser.process(&data);
                    let msg = ServerMsg::Output(data);
                    for client in clients.values_mut() {
                        let _ = protocol::write_server_msg(&mut client.writer, &msg);
                    }
                }

                // Get exit code
                let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(0);

                // Broadcast Exited to all clients
                let exit_msg = ServerMsg::Exited(code);
                for client in clients.values_mut() {
                    let _ = protocol::write_server_msg(&mut client.writer, &exit_msg);
                }

                // Cleanup happens via CleanupGuard on drop
                break;
            }
            Err(_) => {
                // All senders dropped
                break;
            }
        }
    }

    Ok(())
}

/// Recalculate effective PTY size = min(cols), min(rows) across all connected clients
/// that have sent at least one Resize. Resize PTY + parser if changed.
fn recalc_size(
    clients: &HashMap<u64, ClientEntry>,
    pty: &pty_process::blocking::Pty,
    parser: &mut vt100::Parser,
    pty_cols: &mut u16,
    pty_rows: &mut u16,
) {
    let resized_clients: Vec<&ClientEntry> = clients.values().filter(|c| c.has_resized).collect();
    if resized_clients.is_empty() {
        return;
    }

    let new_cols = resized_clients.iter().map(|c| c.cols).min().unwrap_or(80);
    let new_rows = resized_clients.iter().map(|c| c.rows).min().unwrap_or(24);

    if new_cols != *pty_cols || new_rows != *pty_rows {
        *pty_cols = new_cols;
        *pty_rows = new_rows;
        let _ = set_pty_size(pty, new_rows, new_cols);
        parser.set_size(new_rows, new_cols);
    }
}

/// Recalculate effective size and broadcast Resized to all clients if changed.
fn recalc_size_and_broadcast(
    clients: &mut HashMap<u64, ClientEntry>,
    pty: &pty_process::blocking::Pty,
    parser: &mut vt100::Parser,
    pty_cols: &mut u16,
    pty_rows: &mut u16,
) {
    let resized_clients: Vec<&ClientEntry> = clients.values().filter(|c| c.has_resized).collect();
    if resized_clients.is_empty() {
        return;
    }

    let new_cols = resized_clients.iter().map(|c| c.cols).min().unwrap_or(80);
    let new_rows = resized_clients.iter().map(|c| c.rows).min().unwrap_or(24);

    if new_cols != *pty_cols || new_rows != *pty_rows {
        *pty_cols = new_cols;
        *pty_rows = new_rows;
        let _ = set_pty_size(pty, new_rows, new_cols);
        parser.set_size(new_rows, new_cols);

        // Broadcast Resized to all clients
        let msg = ServerMsg::Resized {
            cols: new_cols,
            rows: new_rows,
        };
        let mut disconnected = Vec::new();
        for (&id, client) in clients.iter_mut() {
            if protocol::write_server_msg(&mut client.writer, &msg).is_err() {
                disconnected.push(id);
            }
        }
        for id in disconnected {
            clients.remove(&id);
        }
    }
}
