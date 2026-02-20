mod client;
mod protocol;
pub mod server;
mod terminal;

use anyhow::{Context, Result};
use std::io::Read;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::mpsc;
use std::time::Duration;

use crate::session;

use terminal::{scrollback_line_count, InputAction, InputState, RawModeGuard, ScrollState};

/// Acquire an exclusive lock on a session-specific lockfile.
/// Returns the lock file (must be kept alive for the duration of the lock).
fn acquire_session_lock(session_name: &str) -> Result<std::fs::File> {
    let dir = session::sessions_dir()?.join(session_name);
    std::fs::create_dir_all(&dir)?;
    let lock_path = dir.join("lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .context("Failed to open session lock file")?;
    let fd = lock_file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if ret != 0 {
        anyhow::bail!("Failed to acquire session lock");
    }
    Ok(lock_file)
}

/// Maximum lines of scrollback history kept per terminal.
const SCROLLBACK_LINES: usize = 10_000;

pub struct MuxConfig {
    pub session_name: String,
    pub command: Vec<String>,
    pub working_dir: Option<String>,
    pub prefix_key: u8,
}

/// Client-server mode for local sessions.
/// Starts server if not running, then attaches as client.
pub fn run(session_name: &str) -> Result<i32> {
    let socket_path = session::socket_path(session_name)?;

    // Try connecting to existing server (fast path, no lock needed)
    if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
        return client::run(session_name, &socket_path);
    }

    // Acquire exclusive lock to prevent two concurrent callers from both
    // spawning a server for the same session (TOCTOU race).
    let _lock = acquire_session_lock(session_name)?;

    // Re-check under the lock — another caller may have started the server
    // while we were waiting for the lock.
    if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
        return client::run(session_name, &socket_path);
    }

    // Kill any stale server process (e.g. one that survived SIGHUP but
    // whose socket was already cleaned up by a previous run).
    kill_stale_server(session_name);

    // Clean stale socket
    let _ = std::fs::remove_file(&socket_path);

    // Spawn server daemon
    spawn_server(session_name)?;

    // Poll for socket (up to 3s), then connect as client
    // Lock is released here (dropped at end of scope) once server is ready.
    wait_for_socket(session_name, &socket_path)?;
    client::run(session_name, &socket_path)
}

fn project_name_for_session(session_name: &str) -> String {
    session::load(session_name)
        .ok()
        .and_then(|s| {
            std::path::Path::new(&s.project_dir)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_default()
}

/// Single-process mode (current behavior). For cmd_exec and Docker.
pub fn run_standalone(config: MuxConfig) -> Result<i32> {
    // Try to open /dev/tty for direct terminal access
    let tty_result = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty");

    let mut tty = match tty_result {
        Ok(f) => f,
        Err(_) => return run_fallback(&config),
    };
    let tty_fd = tty.as_raw_fd();

    let (term_cols, term_rows) = match terminal::get_term_size(tty_fd) {
        Ok(size) => size,
        Err(_) => return run_fallback(&config),
    };

    // Verify termios
    {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut termios) } != 0 {
            return run_fallback(&config);
        }
    }

    let inner_rows = term_rows.saturating_sub(1);
    if inner_rows == 0 || term_cols == 0 {
        anyhow::bail!("Terminal too small");
    }

    // Open PTY
    let pty = pty_process::blocking::Pty::new().context("Failed to open PTY")?;
    let pts = pty.pts().context("Failed to get PTY slave")?;
    terminal::set_pty_size(&pty, inner_rows, term_cols)?;

    // Build command
    let mut cmd = pty_process::blocking::Command::new(&config.command[0]);
    cmd.args(&config.command[1..]);
    cmd.env("BOX_SESSION", &config.session_name);
    cmd.env_remove("__BOX_MUX_SERVER");
    if let Some(ref dir) = config.working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn(&pts).context("Failed to spawn command in PTY")?;
    // Drop pts so the parent doesn't hold the slave side open.
    // Without this, the master never gets EOF when the child exits.
    drop(pts);

    // Create vt100 parser with scrollback
    let mut parser = vt100::Parser::new(inner_rows, term_cols, SCROLLBACK_LINES);

    // Install panic hook
    terminal::install_panic_hook();

    // Enter raw mode
    let _guard = RawModeGuard::activate(&mut tty)?;

    let mut term = terminal::create_terminal(tty_fd, term_cols, term_rows)?;

    let (tx, rx) = mpsc::channel::<StandaloneEvent>();

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
                    let _ = tx_pty.send(StandaloneEvent::ChildExited);
                    break;
                }
                Ok(n) => {
                    if tx_pty
                        .send(StandaloneEvent::PtyOutput(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
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
                        .send(StandaloneEvent::InputBytes(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    let project_name = project_name_for_session(&config.session_name);
    let mut input_state = InputState::new(config.prefix_key);
    let mut dirty = true;
    let mut child_exited = false;
    let mut mouse_tracking_on = false;

    let mut last_cols = term_cols;
    let mut last_rows = term_rows;
    let mut current_inner_rows = inner_rows;

    loop {
        // When dirty, use a short timeout to coalesce rapid output bursts
        // (e.g. a TUI app's multi-chunk SIGWINCH redraw) into a single
        // draw, instead of rendering every intermediate frame.
        let timeout = if dirty {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(50)
        };
        let event = rx.recv_timeout(timeout);
        match event {
            Ok(StandaloneEvent::PtyOutput(data)) => {
                parser.process(&data);
                dirty = true;
            }
            Ok(StandaloneEvent::InputBytes(data)) => {
                let max_scrollback = scrollback_line_count(&mut parser);
                let actions =
                    input_state.process(&data, current_inner_rows, last_cols, max_scrollback);
                for action in actions {
                    match action {
                        InputAction::Forward(bytes) => {
                            let _ = terminal::write_bytes_to_pty(&pty, &bytes);
                        }
                        InputAction::Detach => {
                            // In standalone mode there's no server to keep the
                            // session alive, so kill the child to avoid orphans/zombies.
                            let _ = child.kill();
                            let _ = child.wait();
                            return Ok(0);
                        }
                        InputAction::Kill => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Ok(0);
                        }
                        InputAction::Redraw => {
                            dirty = true;
                        }
                    }
                }
            }
            Ok(StandaloneEvent::ChildExited) => {
                child_exited = true;
                while let Ok(StandaloneEvent::PtyOutput(data)) = rx.try_recv() {
                    parser.process(&data);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Flush any buffered incomplete escape sequence
                // (e.g. bare ESC that wasn't followed by more bytes).
                let max_scrollback = scrollback_line_count(&mut parser);
                let pending_actions =
                    input_state.flush_pending(current_inner_rows, last_cols, max_scrollback);
                for action in pending_actions {
                    match action {
                        InputAction::Forward(bytes) => {
                            let _ = terminal::write_bytes_to_pty(&pty, &bytes);
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
                            let _ = terminal::set_pty_size(&pty, new_inner, cols);
                            parser.set_size(new_inner, cols);
                            // Clear parser screen — set_size() rewraps old
                            // content which looks garbled for TUI apps.
                            // The child will send a fresh redraw via SIGWINCH.
                            parser.process(b"\x1b[H\x1b[2J");
                            term = terminal::create_terminal(tty_fd, cols, rows)?;
                            // Clear stale content left by the terminal emulator's
                            // resize reflow.  Without this, ratatui's diff skips
                            // "empty" cells that still show old content on screen.
                            term.clear()?;
                        }
                        input_state.scroll_offset = 0;
                        dirty = true;
                    }
                }

                // Draw only when the event queue is quiet, so rapid bursts
                // of output are coalesced into a single frame.
                if dirty {
                    let max_scrollback = scrollback_line_count(&mut parser);

                    // Enable mouse tracking only when there's scrollback content
                    let want_mouse = true;
                    if want_mouse != mouse_tracking_on {
                        mouse_tracking_on = want_mouse;
                        terminal::set_mouse_tracking(tty_fd, mouse_tracking_on);
                    }

                    parser.set_scrollback(input_state.scroll_offset);
                    let session_name = config.session_name.clone();
                    let project_name = project_name.clone();
                    let screen = parser.screen();
                    let scroll = ScrollState {
                        offset: input_state.scroll_offset,
                        max: max_scrollback,
                    };
                    let cmd_mode = input_state.command_mode;
                    let hover_close = input_state.hover_close;
                    term.draw(|f| {
                        terminal::draw_frame(
                            f,
                            screen,
                            &session_name,
                            &project_name,
                            &scroll,
                            cmd_mode,
                            hover_close,
                        );
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

    // Always reap the child to avoid zombies.
    // If it's still running (e.g. channel disconnected), kill it first.
    if !child_exited {
        let _ = child.kill();
    }
    let status = child.wait().ok();

    let exit_code = status.and_then(|s| s.code()).unwrap_or(0);
    Ok(exit_code)
}

/// Send Kill to a running server. For `box stop`.
pub fn send_kill(session_name: &str) -> Result<()> {
    let socket_path = session::socket_path(session_name)?;
    let mut sock = std::os::unix::net::UnixStream::connect(&socket_path)
        .context("Failed to connect to mux server")?;
    protocol::write_client_msg(&mut sock, &protocol::ClientMsg::Kill)?;

    // Wait for server to shut down (up to 5s)
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        if std::os::unix::net::UnixStream::connect(&socket_path).is_err() {
            return Ok(());
        }
    }

    Ok(())
}

// --- Private helpers ---

fn spawn_server(session_name: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    // Redirect server stderr to a log file for debugging (append mode)
    let log_path = session::sessions_dir()?
        .join(session_name)
        .join("server.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to create server log: {}", log_path.display()))?;

    unsafe {
        Command::new(exe)
            .env("__BOX_MUX_SERVER", session_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .pre_exec(|| {
                // On Linux, use setsid() to create a new session so the child
                // PTY process can properly claim a controlling terminal, which
                // is required for job control (Ctrl+C, Ctrl+Z).
                //
                // On macOS, we avoid setsid() because being a session leader
                // causes macOS to auto-assign the PTY slave as the server's
                // controlling terminal when opened, which then prevents the
                // child from claiming it via TIOCSCTTY. Instead, use setpgid()
                // to detach from the caller's process group.
                #[cfg(target_os = "linux")]
                {
                    libc::setsid();
                }
                #[cfg(not(target_os = "linux"))]
                {
                    libc::setpgid(0, 0);
                }
                // Ignore SIGHUP so the server survives terminal close.
                // This is set before exec() so it persists into the new process.
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            })
            .spawn()
            .context("Failed to spawn mux server")?;
    }

    Ok(())
}

fn wait_for_socket(session_name: &str, socket_path: &std::path::Path) -> Result<()> {
    for _ in 0..60 {
        if socket_path.exists() && std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Print server log for debugging
    if let Ok(dir) = session::sessions_dir() {
        let log_path = dir.join(session_name).join("server.log");
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            if !log.is_empty() {
                eprintln!("Server log:\n{}", log);
            }
        }
    }
    anyhow::bail!("Timed out waiting for mux server to start")
}

/// Kill a stale server process for this session (if any) via its PID file.
/// This prevents orphaned server processes when a server dies from a signal
/// but its PID file was not cleaned up.
fn kill_stale_server(session_name: &str) {
    if let Ok(dir) = session::sessions_dir() {
        let pid_path = dir.join(session_name).join("pid");
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                if pid > 0 && is_box_mux_server(pid) {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                    // Brief wait for the process to be reaped
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        // Remove stale PID file
        let _ = std::fs::remove_file(&pid_path);
    }
}

/// Check whether the given PID belongs to a box mux server process.
/// Returns false if the PID doesn't exist or belongs to an unrelated process,
/// preventing accidental kills of recycled PIDs.
fn is_box_mux_server(pid: i32) -> bool {
    // First check if the process is alive at all
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }

    // On macOS, use `ps` to verify the process command contains our binary name.
    // On Linux, check /proc/<pid>/cmdline.
    #[cfg(target_os = "linux")]
    {
        let cmdline_path = format!("/proc/{}/cmdline", pid);
        if let Ok(cmdline) = std::fs::read_to_string(&cmdline_path) {
            return cmdline.contains("box") && cmdline.contains("__BOX_MUX_SERVER");
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        // On macOS/BSD, check /proc is unavailable; use `ps` to verify.
        // Look for the __BOX_MUX_SERVER env marker in the process environment
        // via `ps eww` which shows environment variables on macOS.
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
        {
            let cmd = String::from_utf8_lossy(&output.stdout);
            return cmd.contains("box");
        }
        false
    }
}

enum StandaloneEvent {
    PtyOutput(Vec<u8>),
    InputBytes(Vec<u8>),
    ChildExited,
}

/// Fallback: run command with inherited stdio (no mux chrome).
fn run_fallback(config: &MuxConfig) -> Result<i32> {
    let mut child = std::process::Command::new(&config.command[0])
        .args(&config.command[1..])
        .current_dir(config.working_dir.as_deref().unwrap_or("."))
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .with_context(|| format!("Failed to run command: {}", config.command.join(" ")))?;
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}
