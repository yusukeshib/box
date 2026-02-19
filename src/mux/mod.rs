mod client;
mod protocol;
pub mod server;

use anyhow::{Context, Result};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::mpsc;
use std::time::Duration;

use crate::session;

pub struct MuxConfig {
    pub session_name: String,
    pub command: Vec<String>,
    pub working_dir: Option<String>,
}

/// Client-server mode for local sessions.
/// Starts server if not running, then attaches as client.
pub fn run(session_name: &str) -> Result<i32> {
    let socket_path = session::socket_path(session_name)?;

    // Try connecting to existing server
    if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
        return client::run(session_name, &socket_path);
    }

    // Clean stale socket
    let _ = std::fs::remove_file(&socket_path);

    // Spawn server daemon
    spawn_server(session_name)?;

    // Poll for socket (up to 3s), then connect as client
    wait_for_socket(session_name, &socket_path)?;
    client::run(session_name, &socket_path)
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

    let (term_cols, term_rows) = match get_term_size(tty_fd) {
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
    set_pty_size(&pty, inner_rows, term_cols)?;

    // Build command
    let mut cmd = pty_process::blocking::Command::new(&config.command[0]);
    cmd.args(&config.command[1..]);
    if let Some(ref dir) = config.working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn(&pts).context("Failed to spawn command in PTY")?;

    // Create vt100 parser with scrollback
    let mut parser = vt100::Parser::new(inner_rows, term_cols, 10_000);

    // Install panic hook
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = tty.write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
            let _ = std::process::Command::new("stty")
                .arg("sane")
                .stdin(unsafe { std::process::Stdio::from_raw_fd(tty.as_raw_fd()) })
                .status();
        }
        default_hook(info);
    }));

    // Enter raw mode
    let _guard = RawModeGuard::activate(&mut tty)?;

    let tty_write_fd = unsafe { libc::dup(tty_fd) };
    if tty_write_fd < 0 {
        anyhow::bail!("Failed to dup tty fd: {}", io::Error::last_os_error());
    }
    let tty_writer = unsafe { std::fs::File::from_raw_fd(tty_write_fd) };
    let backend = CrosstermBackend::new(tty_writer);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, term_cols, term_rows)),
        },
    )
    .context("Failed to create terminal")?;

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
                    let _ = tx_pty.send(StandaloneEvent::PtyOutput(buf[..n].to_vec()));
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

    let mut prefix_active = false;
    let mut scroll_offset: usize = 0;
    let mut scrollback_mode = false;
    let mut show_help = false;
    let mut dirty = true;
    let mut child_exited = false;

    let mut last_cols = term_cols;
    let mut last_rows = term_rows;

    loop {
        if dirty {
            parser.set_scrollback(scroll_offset);
            let session_name = config.session_name.clone();
            let screen = parser.screen();
            let is_scrollback = scrollback_mode;
            terminal
                .draw(|f| {
                    let area = f.area();

                    let header_area = Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: 1,
                    };

                    let grid_area = Rect {
                        x: area.x,
                        y: area.y + 1,
                        width: area.width,
                        height: area.height.saturating_sub(1),
                    };

                    let header_style = Style::default().bg(Color::White).fg(Color::Black);
                    let left = format!(" {} ", session_name);
                    let right = if show_help {
                        " Ctrl+B d:detach  x:stop  [:scroll  ?:help "
                    } else if is_scrollback {
                        " SCROLL: Up/Down PgUp/PgDn  q:exit "
                    } else {
                        " Ctrl+B ? for help "
                    };

                    let pad = (area.width as usize)
                        .saturating_sub(left.len())
                        .saturating_sub(right.len());

                    let header_text = format!("{}{}{}", left, " ".repeat(pad), right);
                    let header = Paragraph::new(header_text).style(header_style);
                    f.render_widget(header, header_area);

                    let widget = TerminalWidget {
                        screen,
                        show_cursor: !is_scrollback,
                    };
                    f.render_widget(widget, grid_area);
                })
                .context("Failed to draw terminal frame")?;
            parser.set_scrollback(0);
            dirty = false;
        }

        let event = rx.recv_timeout(Duration::from_millis(50));
        match event {
            Ok(StandaloneEvent::PtyOutput(data)) => {
                parser.process(&data);
                if !scrollback_mode {
                    scroll_offset = 0;
                }
                dirty = true;
            }
            Ok(StandaloneEvent::InputBytes(data)) => {
                let mut i = 0;
                while i < data.len() {
                    let b = data[i];

                    if scrollback_mode {
                        if b == 0x1b && i + 2 < data.len() && data[i + 1] == b'[' {
                            match data[i + 2] {
                                b'A' => {
                                    let max_scroll = parser.screen().scrollback();
                                    if scroll_offset < max_scroll {
                                        scroll_offset += 1;
                                    }
                                    dirty = true;
                                    i += 3;
                                    continue;
                                }
                                b'B' => {
                                    scroll_offset = scroll_offset.saturating_sub(1);
                                    if scroll_offset == 0 {
                                        scrollback_mode = false;
                                    }
                                    dirty = true;
                                    i += 3;
                                    continue;
                                }
                                b'5' if i + 3 < data.len() && data[i + 3] == b'~' => {
                                    let half = (inner_rows / 2) as usize;
                                    let max_scroll = parser.screen().scrollback();
                                    scroll_offset = (scroll_offset + half).min(max_scroll);
                                    dirty = true;
                                    i += 4;
                                    continue;
                                }
                                b'6' if i + 3 < data.len() && data[i + 3] == b'~' => {
                                    let half = (inner_rows / 2) as usize;
                                    scroll_offset = scroll_offset.saturating_sub(half);
                                    if scroll_offset == 0 {
                                        scrollback_mode = false;
                                    }
                                    dirty = true;
                                    i += 4;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        match b {
                            b'q' | 0x1b => {
                                if b == b'q'
                                    || (b == 0x1b && (i + 1 >= data.len() || data[i + 1] != b'['))
                                {
                                    scrollback_mode = false;
                                    scroll_offset = 0;
                                    dirty = true;
                                    i += 1;
                                    continue;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                        continue;
                    }

                    if prefix_active {
                        prefix_active = false;
                        match b {
                            b'd' => {
                                return Ok(0);
                            }
                            b'x' => {
                                let _ = child.kill();
                                let _ = child.wait();
                                return Ok(0);
                            }
                            b'[' => {
                                scrollback_mode = true;
                                dirty = true;
                                i += 1;
                                continue;
                            }
                            b'?' => {
                                show_help = !show_help;
                                dirty = true;
                                i += 1;
                                continue;
                            }
                            0x02 => {
                                let _ = write_bytes_to_pty(&pty, &[0x02]);
                            }
                            _ => {
                                let _ = write_bytes_to_pty(&pty, &[0x02, b]);
                            }
                        }
                        i += 1;
                        continue;
                    }

                    if b == 0x02 {
                        prefix_active = true;
                        i += 1;
                        continue;
                    }

                    let start = i;
                    while i < data.len() && data[i] != 0x02 {
                        i += 1;
                    }
                    let _ = write_bytes_to_pty(&pty, &data[start..i]);
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
                if let Ok((cols, rows)) = get_term_size(tty_fd) {
                    if cols != last_cols || rows != last_rows {
                        last_cols = cols;
                        last_rows = rows;
                        let new_inner = rows.saturating_sub(1);
                        if new_inner > 0 && cols > 0 {
                            let _ = set_pty_size(&pty, new_inner, cols);
                            parser.set_size(new_inner, cols);
                            let _ = terminal.resize(Rect::new(0, 0, cols, rows));
                        }
                        dirty = true;
                    }
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    let status = if child_exited {
        child.wait().ok()
    } else {
        None
    };

    let exit_code = status.and_then(|s| s.code()).unwrap_or(0);
    Ok(exit_code)
}

/// Send Kill to a running server. For `box stop`.
pub fn send_kill(session_name: &str) -> Result<()> {
    let socket_path = session::socket_path(session_name)?;
    let mut sock = std::os::unix::net::UnixStream::connect(&socket_path)
        .context("Failed to connect to mux server")?;
    protocol::write_client_msg(&mut sock, &protocol::ClientMsg::Kill)?;
    Ok(())
}

// --- Private helpers ---

fn spawn_server(session_name: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    // Redirect server stderr to a log file for debugging
    let log_path = session::sessions_dir()?
        .join(session_name)
        .join("server.log");
    let log_file = std::fs::File::create(&log_path)
        .with_context(|| format!("Failed to create server log: {}", log_path.display()))?;

    unsafe {
        Command::new(exe)
            .env("__BOX_MUX_SERVER", session_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .pre_exec(|| {
                // Put server in its own process group so it doesn't receive
                // signals from the caller's terminal. We avoid setsid() because
                // being a session leader causes macOS to auto-assign the PTY
                // slave as our controlling terminal when opened, which then
                // prevents the child from claiming it via TIOCSCTTY.
                libc::setpgid(0, 0);
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

enum StandaloneEvent {
    PtyOutput(Vec<u8>),
    InputBytes(Vec<u8>),
    ChildExited,
}

/// RAII guard that restores terminal state on drop (including panics).
struct RawModeGuard {
    tty_fd: i32,
    orig_termios: libc::termios,
}

impl RawModeGuard {
    fn activate(tty: &mut std::fs::File) -> Result<Self> {
        let tty_fd = tty.as_raw_fd();

        let mut orig_termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut orig_termios) } != 0 {
            anyhow::bail!(
                "Failed to get terminal attributes: {}",
                io::Error::last_os_error()
            );
        }

        let mut raw = orig_termios;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(tty_fd, libc::TCSANOW, &raw) } != 0 {
            anyhow::bail!("Failed to set raw mode: {}", io::Error::last_os_error());
        }

        tty.write_all(b"\x1b[?1049h\x1b[?25l")?;
        tty.flush()?;

        Ok(RawModeGuard {
            tty_fd,
            orig_termios,
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = tty.write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
        }
        unsafe {
            libc::tcsetattr(self.tty_fd, libc::TCSANOW, &self.orig_termios);
        }
    }
}

/// Custom ratatui Widget that renders a vt100::Screen.
struct TerminalWidget<'a> {
    screen: &'a vt100::Screen,
    show_cursor: bool,
}

impl<'a> Widget for TerminalWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let rows = self.screen.size().0 as usize;
        let cols = self.screen.size().1 as usize;

        let cursor_pos = if self.show_cursor && !self.screen.hide_cursor() {
            Some(self.screen.cursor_position())
        } else {
            None
        };

        for y in 0..area.height as usize {
            if y >= rows {
                break;
            }
            for x in 0..area.width as usize {
                if x >= cols {
                    break;
                }
                let buf_x = area.x + x as u16;
                let buf_y = area.y + y as u16;
                if buf_x >= buf.area().width || buf_y >= buf.area().height {
                    continue;
                }

                let Some(cell) = self.screen.cell(y as u16, x as u16) else {
                    continue;
                };

                let ch: String = cell.contents();
                let display_ch = if ch.is_empty() { " ".to_string() } else { ch };

                let mut style = Style::default();
                style = style.fg(map_vt100_color(cell.fgcolor()));
                style = style.bg(map_vt100_color(cell.bgcolor()));

                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                if let Some((cr, cc)) = cursor_pos {
                    if y == cr as usize && x == cc as usize {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                }

                let buf_cell = &mut buf[(buf_x, buf_y)];
                buf_cell.set_symbol(&display_ch);
                buf_cell.set_style(style);
            }
        }
    }
}

fn map_vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Get terminal size via ioctl on a given fd.
fn get_term_size(fd: i32) -> Result<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) };
    if ret != 0 {
        anyhow::bail!(
            "Failed to get terminal size: {}",
            io::Error::last_os_error()
        );
    }
    Ok((size.ws_col, size.ws_row))
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
