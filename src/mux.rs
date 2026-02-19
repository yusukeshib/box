use anyhow::{Context, Result};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::sync::mpsc;
use std::time::Duration;

pub struct MuxConfig {
    pub session_name: String,
    pub command: Vec<String>,
    pub working_dir: Option<String>,
}

enum MuxEvent {
    PtyOutput(Vec<u8>),
    /// Raw bytes from the terminal input
    InputBytes(Vec<u8>),
    ChildExited,
}

/// RAII guard that restores terminal state on drop (including panics).
/// Uses /dev/tty so cleanup works even when stdin/stdout are redirected.
struct RawModeGuard {
    tty_fd: i32,
    orig_termios: libc::termios,
}

impl RawModeGuard {
    fn activate(tty: &mut std::fs::File) -> Result<Self> {
        let tty_fd = tty.as_raw_fd();

        // Save original termios
        let mut orig_termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut orig_termios) } != 0 {
            anyhow::bail!(
                "Failed to get terminal attributes: {}",
                io::Error::last_os_error()
            );
        }

        // Set raw mode
        let mut raw = orig_termios;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(tty_fd, libc::TCSANOW, &raw) } != 0 {
            anyhow::bail!("Failed to set raw mode: {}", io::Error::last_os_error());
        }

        // Enter alternate screen, hide cursor
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
        // Best-effort cleanup via /dev/tty
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            // Show cursor, leave alternate screen, reset attributes
            let _ = tty.write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
        }
        // Restore original termios
        unsafe {
            libc::tcsetattr(self.tty_fd, libc::TCSANOW, &self.orig_termios);
        }
    }
}

/// Custom ratatui Widget that renders a vt100::Screen.
/// The screen's scrollback offset must be set before rendering via
/// `parser.set_scrollback(offset)`, so `screen.cell()` returns the right cells.
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

pub fn run(config: MuxConfig) -> Result<i32> {
    // Try to open /dev/tty for direct terminal access. If that fails (e.g. no
    // controlling terminal, inside CI, piped context), fall back to plain exec.
    let tty_result = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty");

    let mut tty = match tty_result {
        Ok(f) => f,
        Err(_) => return run_fallback(&config),
    };
    let tty_fd = tty.as_raw_fd();

    // Get terminal size from the tty fd — if this fails the tty isn't usable.
    let (term_cols, term_rows) = match get_term_size(tty_fd) {
        Ok(size) => size,
        Err(_) => return run_fallback(&config),
    };

    // Verify we can get termios (i.e. this is a real terminal device).
    // If not, fall back to plain exec before we spawn anything.
    {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut termios) } != 0 {
            return run_fallback(&config);
        }
    }

    let inner_rows = term_rows.saturating_sub(1); // reserve 1 row for header
    if inner_rows == 0 || term_cols == 0 {
        anyhow::bail!("Terminal too small");
    }

    // Open PTY and resize
    let pty = pty_process::blocking::Pty::new().context("Failed to open PTY")?;
    pty.resize(pty_process::Size::new(inner_rows, term_cols))
        .context("Failed to resize PTY")?;

    // Build command
    let mut cmd = pty_process::blocking::Command::new(&config.command[0]);
    cmd.args(&config.command[1..]);
    if let Some(ref dir) = config.working_dir {
        cmd.current_dir(dir);
    }

    let pts = pty.pts().context("Failed to get PTY slave")?;
    let mut child = cmd.spawn(&pts).context("Failed to spawn command in PTY")?;

    // Create vt100 parser with scrollback
    let mut parser = vt100::Parser::new(inner_rows, term_cols, 10_000);

    // Install panic hook to restore terminal
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = tty.write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
            // Restore cooked mode via stty
            let _ = std::process::Command::new("stty")
                .arg("sane")
                .stdin(unsafe { std::process::Stdio::from_raw_fd(tty.as_raw_fd()) })
                .status();
        }
        default_hook(info);
    }));

    // Enter raw mode + alternate screen using /dev/tty
    let _guard = RawModeGuard::activate(&mut tty)?;

    // Create a separate writable handle for the ratatui backend.
    let tty_write_fd = unsafe { libc::dup(tty_fd) };
    if tty_write_fd < 0 {
        anyhow::bail!("Failed to dup tty fd: {}", io::Error::last_os_error());
    }
    let tty_writer = unsafe { std::fs::File::from_raw_fd(tty_write_fd) };
    let backend = CrosstermBackend::new(tty_writer);
    // Use Viewport::Fixed to avoid crossterm::terminal::size() calls, which can
    // fail with ENOTTY. We already know the size from our own ioctl on /dev/tty.
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, term_cols, term_rows)),
        },
    )
    .context("Failed to create terminal")?;

    // Channel for events
    let (tx, rx) = mpsc::channel::<MuxEvent>();

    // Spawn PTY reader thread
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
                    let _ = tx_pty.send(MuxEvent::ChildExited);
                    break;
                }
                Ok(n) => {
                    let _ = tx_pty.send(MuxEvent::PtyOutput(buf[..n].to_vec()));
                }
            }
        }
    });

    // Spawn input reader thread — reads raw bytes from /dev/tty.
    // We forward raw bytes directly to the PTY (the terminal does the
    // encoding). We only intercept Ctrl+B (0x02) for our prefix commands.
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
                        .send(MuxEvent::InputBytes(buf[..n].to_vec()))
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

    // For detecting SIGWINCH (terminal resize), we poll the tty size
    // periodically since we can't use crossterm's event system.
    let mut last_cols = term_cols;
    let mut last_rows = term_rows;

    // Main event loop
    loop {
        // Render when dirty
        if dirty {
            // Set scrollback offset so screen.cell() returns the right view
            parser.set_scrollback(scroll_offset);
            let session_name = config.session_name.clone();
            let screen = parser.screen();
            let is_scrollback = scrollback_mode;
            terminal
                .draw(|f| {
                    let area = f.area();

                    // Header bar (1 row)
                    let header_area = Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: 1,
                    };

                    // Terminal grid (remaining rows)
                    let grid_area = Rect {
                        x: area.x,
                        y: area.y + 1,
                        width: area.width,
                        height: area.height.saturating_sub(1),
                    };

                    // Build header
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

                    // Render terminal grid
                    let widget = TerminalWidget {
                        screen,
                        show_cursor: !is_scrollback,
                    };
                    f.render_widget(widget, grid_area);
                })
                .context("Failed to draw terminal frame")?;
            // Reset scrollback offset so parser.process() works normally
            parser.set_scrollback(0);
            dirty = false;
        }

        // Wait for events
        let event = rx.recv_timeout(Duration::from_millis(50));
        match event {
            Ok(MuxEvent::PtyOutput(data)) => {
                parser.process(&data);
                if !scrollback_mode {
                    scroll_offset = 0;
                }
                dirty = true;
            }
            Ok(MuxEvent::InputBytes(data)) => {
                // Process raw input bytes, intercepting Ctrl+B prefix
                let mut i = 0;
                while i < data.len() {
                    let b = data[i];

                    if scrollback_mode {
                        // In scrollback mode, interpret escape sequences for navigation
                        if b == 0x1b && i + 2 < data.len() && data[i + 1] == b'[' {
                            match data[i + 2] {
                                b'A' => {
                                    // Up arrow
                                    let max_scroll = parser.screen().scrollback();
                                    if scroll_offset < max_scroll {
                                        scroll_offset += 1;
                                    }
                                    dirty = true;
                                    i += 3;
                                    continue;
                                }
                                b'B' => {
                                    // Down arrow
                                    scroll_offset = scroll_offset.saturating_sub(1);
                                    if scroll_offset == 0 {
                                        scrollback_mode = false;
                                    }
                                    dirty = true;
                                    i += 3;
                                    continue;
                                }
                                b'5' if i + 3 < data.len() && data[i + 3] == b'~' => {
                                    // Page Up
                                    let half = (inner_rows / 2) as usize;
                                    let max_scroll = parser.screen().scrollback();
                                    scroll_offset = (scroll_offset + half).min(max_scroll);
                                    dirty = true;
                                    i += 4;
                                    continue;
                                }
                                b'6' if i + 3 < data.len() && data[i + 3] == b'~' => {
                                    // Page Down
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
                                // q or Esc: exit scrollback
                                // But only bare Esc (not part of a sequence)
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
                                // Detach
                                return Ok(detach_exit_code());
                            }
                            b'x' => {
                                // Stop - kill child
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
                                // Ctrl+B Ctrl+B -> send literal Ctrl+B
                                let _ = write_bytes_to_pty(&pty, &[0x02]);
                            }
                            _ => {
                                // Not a recognized prefix command — send Ctrl+B + the byte
                                let _ = write_bytes_to_pty(&pty, &[0x02, b]);
                            }
                        }
                        i += 1;
                        continue;
                    }

                    if b == 0x02 {
                        // Ctrl+B — activate prefix
                        prefix_active = true;
                        i += 1;
                        continue;
                    }

                    // Normal input — find the next Ctrl+B (if any) and forward
                    // everything before it to the PTY in one write.
                    let start = i;
                    while i < data.len() && data[i] != 0x02 {
                        i += 1;
                    }
                    let _ = write_bytes_to_pty(&pty, &data[start..i]);
                }
            }
            Ok(MuxEvent::ChildExited) => {
                child_exited = true;
                // Drain remaining output
                while let Ok(MuxEvent::PtyOutput(data)) = rx.try_recv() {
                    parser.process(&data);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check for terminal resize
                if let Ok((cols, rows)) = get_term_size(tty_fd) {
                    if cols != last_cols || rows != last_rows {
                        last_cols = cols;
                        last_rows = rows;
                        let new_inner = rows.saturating_sub(1);
                        if new_inner > 0 && cols > 0 {
                            let _ = pty.resize(pty_process::Size::new(new_inner, cols));
                            parser.set_size(new_inner, cols);
                            // Update ratatui's fixed viewport area
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

    // Wait for child
    let status = if child_exited {
        child.wait().ok()
    } else {
        // Detach case - don't wait
        None
    };

    let exit_code = status.and_then(|s| s.code()).unwrap_or(0);

    Ok(exit_code)
}

/// Exit code returned when detaching. The caller can use this to distinguish
/// detach from normal exit if needed.
fn detach_exit_code() -> i32 {
    0
}

/// Fallback: run command with inherited stdio (no mux chrome).
/// Used when /dev/tty is unavailable or not a real terminal.
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

fn write_bytes_to_pty(pty: &pty_process::blocking::Pty, data: &[u8]) -> Result<()> {
    use std::os::unix::io::AsFd;
    let fd = pty.as_fd();
    let raw_fd = fd.as_raw_fd();
    // Write directly via fd to avoid borrow issues
    let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
    let result = file.write_all(data);
    // Don't close the fd - it belongs to the PTY
    std::mem::forget(file);
    result.map_err(Into::into)
}
