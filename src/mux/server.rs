use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::config;
use crate::session;

use super::protocol::{self, ClientMsg, ServerMsg};
use super::terminal;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

enum ServerEvent {
    PtyOutput(Vec<u8>),
    NewClient(UnixStream),
    ClientMsg { id: u64, msg: ClientMsg },
    ClientDisconnected(u64),
    ChildExited,
}

struct ClientEntry {
    /// Bounded channel to per-client writer thread.  Sending serialised
    /// message bytes here never blocks the server event loop.
    tx: mpsc::SyncSender<Arc<[u8]>>,
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

pub fn run(session_name: &str) -> Result<()> {
    // Install signal handlers
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_sigterm as *const () as libc::sighandler_t,
        );
        // Ignore SIGHUP so the server survives when the spawning terminal closes
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    // Load session metadata
    let sess = session::load(session_name)?;
    if sess.command.is_empty() {
        anyhow::bail!("Session '{}' has no command configured.", session_name);
    }

    // Derive workspace path (use workspace name, not full session name)
    let home = config::home_dir()?;
    let workspace = Path::new(&home)
        .join(".box")
        .join("workspaces")
        .join(session::workspace_name(session_name));

    // Ensure session directory has restricted permissions before creating socket
    let sess_dir = session::sessions_dir()?.join(session_name);
    if sess_dir.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sess_dir, std::fs::Permissions::from_mode(0o700));
        }
    }

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
    terminal::set_pty_size(&pty, default_rows, default_cols)?;

    // Spawn child
    let mut cmd = pty_process::blocking::Command::new(&sess.command[0]);
    cmd.args(&sess.command[1..]);
    cmd.env("BOX_SESSION", session_name);
    cmd.env_remove("__BOX_MUX_SERVER");
    if workspace.is_dir() {
        cmd.current_dir(&workspace);
    }
    let mut child = cmd
        .spawn(&pts)
        .with_context(|| format!("Failed to spawn {:?} in PTY", &sess.command))?;
    // Drop pts so the server doesn't hold the slave side open
    drop(pts);

    // Create vt100 parser for screen state
    let mut parser = vt100::Parser::new(default_rows, default_cols, super::SCROLLBACK_LINES);

    // Raw PTY output history for replaying scrollback to new clients.
    // Capped at 4MB — enough for ~10k lines of typical terminal output.
    // VecDeque so that draining old bytes from the front is O(1) amortised
    // instead of the O(n) memmove that Vec::drain(..n) requires.
    let mut history: VecDeque<u8> = VecDeque::new();
    const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;

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
                    if tx_pty
                        .send(ServerEvent::PtyOutput(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
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
        let event = rx.recv_timeout(Duration::from_millis(100));
        match event {
            Ok(ServerEvent::PtyOutput(data)) => {
                parser.process(&data);
                // Accumulate raw output for scrollback replay
                history.extend(data.iter().copied());
                if history.len() > MAX_HISTORY_BYTES {
                    let excess = history.len() - MAX_HISTORY_BYTES;
                    // Find a newline near the cut point to avoid splitting
                    // mid-escape-sequence, which would garble scrollback for
                    // newly connecting clients.
                    let cut = (excess..history.len())
                        .find(|&i| history[i] == b'\n')
                        .map(|p| p + 1)
                        .unwrap_or(excess);
                    history.drain(..cut);
                }
                // Broadcast to all clients via their writer-thread channels
                let msg_bytes: Arc<[u8]> =
                    Arc::from(protocol::serialize_server_msg(&ServerMsg::Output(data)));
                let mut disconnected = Vec::new();
                for (&id, client) in clients.iter() {
                    if client.tx.try_send(msg_bytes.clone()).is_err() {
                        disconnected.push(id);
                    }
                }
                // Only recalculate PTY size when clients were actually removed
                if !disconnected.is_empty() {
                    for id in disconnected {
                        clients.remove(&id);
                    }
                    if !clients.is_empty() {
                        recalc_size(
                            &mut clients,
                            &pty,
                            &mut parser,
                            &mut pty_cols,
                            &mut pty_rows,
                        );
                    }
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

                // Set a write timeout so the writer thread doesn't block
                // indefinitely on a stuck client socket.
                let mut writer_stream = stream;
                let _ = writer_stream.set_write_timeout(Some(Duration::from_secs(5)));

                // Bounded channel for non-blocking broadcast to this client.
                // Capacity of 64 messages (~256 KB of buffered output).
                let (client_tx, client_rx) = mpsc::sync_channel::<Arc<[u8]>>(64);

                // Per-client writer thread — drains the channel and writes
                // to the socket.  This decouples broadcast from socket I/O
                // so a slow client cannot block output to other clients.
                std::thread::spawn(move || {
                    while let Ok(bytes) = client_rx.recv() {
                        if writer_stream.write_all(&bytes).is_err() {
                            break;
                        }
                    }
                });

                clients.insert(
                    id,
                    ClientEntry {
                        tx: client_tx,
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
                    if cols == 0 || rows == 0 {
                        continue;
                    }
                    if let Some(client) = clients.get_mut(&id) {
                        let first_resize = !client.has_resized;
                        client.cols = cols;
                        client.rows = rows;
                        client.has_resized = true;

                        if first_resize {
                            // Send current PTY size
                            let _ = client.tx.send(Arc::from(protocol::serialize_server_msg(
                                &ServerMsg::Resized {
                                    cols: pty_cols,
                                    rows: pty_rows,
                                },
                            )));
                            // Replay raw PTY history so the client builds up
                            // the same scrollback buffer, then send a formatted
                            // screen dump to ensure the visible area matches exactly.
                            // Always send at least one Output (even if empty) so
                            // the client handshake read doesn't block waiting.
                            //
                            // make_contiguous() arranges the VecDeque in-place
                            // (no heap alloc) so we can serialise from a borrow
                            // instead of cloning the entire history.
                            if !history.is_empty() {
                                let _ = client.tx.send(Arc::from(
                                    protocol::serialize_output_slice(history.make_contiguous()),
                                ));
                            }
                            let contents = parser.screen().contents_formatted();
                            let _ = client.tx.send(Arc::from(protocol::serialize_server_msg(
                                &ServerMsg::Output(contents),
                            )));
                        }

                        // Recalculate effective size
                        recalc_size(
                            &mut clients,
                            &pty,
                            &mut parser,
                            &mut pty_cols,
                            &mut pty_rows,
                        );
                    }
                }
                ClientMsg::Input(data) => {
                    let _ = terminal::write_bytes_to_pty(&pty, &data);
                }
                ClientMsg::Kill => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            },
            Ok(ServerEvent::ClientDisconnected(id)) => {
                clients.remove(&id);
                if !clients.is_empty() {
                    recalc_size(
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
                    let msg_bytes: Arc<[u8]> =
                        Arc::from(protocol::serialize_server_msg(&ServerMsg::Output(data)));
                    for client in clients.values() {
                        let _ = client.tx.try_send(msg_bytes.clone());
                    }
                }

                // Get exit code
                let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(0);

                // Broadcast Exited to all clients
                let exit_bytes: Arc<[u8]> =
                    Arc::from(protocol::serialize_server_msg(&ServerMsg::Exited(code)));
                for client in clients.values() {
                    let _ = client.tx.try_send(exit_bytes.clone());
                }
                // Drop senders so writer threads drain their queues and exit.
                clients.clear();

                // Cleanup happens via CleanupGuard on drop
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check for SIGTERM
                if SHUTDOWN.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = child.wait();

                    // Broadcast Exited to all clients
                    let exit_bytes: Arc<[u8]> =
                        Arc::from(protocol::serialize_server_msg(&ServerMsg::Exited(0)));
                    for client in clients.values() {
                        let _ = client.tx.try_send(exit_bytes.clone());
                    }
                    clients.clear();
                    break;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All senders dropped
                break;
            }
        }
    }

    Ok(())
}

/// Recalculate effective PTY size = min(cols), min(rows) across all connected clients
/// that have sent at least one Resize. Resize PTY + parser if changed, and broadcast
/// the new size to all clients.
fn recalc_size(
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
        let _ = terminal::set_pty_size(pty, new_rows, new_cols);
        parser.set_size(new_rows, new_cols);

        // Broadcast Resized to all clients
        let msg_bytes: Arc<[u8]> = Arc::from(protocol::serialize_server_msg(&ServerMsg::Resized {
            cols: new_cols,
            rows: new_rows,
        }));
        let mut disconnected = Vec::new();
        for (&id, client) in clients.iter() {
            if client.tx.try_send(msg_bytes.clone()).is_err() {
                disconnected.push(id);
            }
        }
        for id in disconnected {
            clients.remove(&id);
        }
    }
}
