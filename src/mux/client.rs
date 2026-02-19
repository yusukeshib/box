use anyhow::{Context, Result};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::protocol::{self, ClientMsg, ServerMsg};

enum ClientEvent {
    ServerMsg(ServerMsg),
    InputBytes(Vec<u8>),
    ServerDisconnected,
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

pub fn run(session_name: &str, socket_path: &Path) -> Result<i32> {
    // Open /dev/tty
    let tty_result = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty");

    let mut tty = match tty_result {
        Ok(f) => f,
        Err(_) => anyhow::bail!("Cannot open /dev/tty for mux client"),
    };
    let tty_fd = tty.as_raw_fd();

    let (term_cols, term_rows) = match get_term_size(tty_fd) {
        Ok(size) => size,
        Err(e) => anyhow::bail!("Cannot get terminal size: {}", e),
    };

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

    // Send initial Resize to server
    protocol::write_client_msg(
        &mut sock_writer,
        &ClientMsg::Resize {
            cols: term_cols,
            rows: inner_rows,
        },
    )?;

    // Wait for Resized from server to know the PTY dimensions
    let mut sock_reader = sock
        .try_clone()
        .context("Failed to clone socket for reader")?;
    let (pty_cols, pty_rows) = match protocol::read_server_msg(&mut sock_reader)? {
        ServerMsg::Resized { cols, rows } => (cols, rows),
        ServerMsg::Exited(code) => return Ok(code),
        _ => (term_cols, inner_rows),
    };

    // Create local parser with server's PTY dimensions
    let mut parser = vt100::Parser::new(pty_rows, pty_cols, 10_000);

    // Process the screen dump that follows
    match protocol::read_server_msg(&mut sock_reader) {
        Ok(ServerMsg::Output(data)) => {
            parser.process(&data);
        }
        Ok(ServerMsg::Exited(code)) => return Ok(code),
        Ok(_) => {}
        Err(_) => return Ok(1),
    }

    // Create ratatui terminal
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

    let mut prefix_active = false;
    let mut scroll_offset: usize = 0;
    let mut scrollback_mode = false;
    let mut show_help = false;
    let mut dirty = true;

    let mut last_cols = term_cols;
    let mut last_rows = term_rows;
    let mut current_inner_rows = inner_rows;

    loop {
        if dirty {
            parser.set_scrollback(scroll_offset);
            let session_name = session_name.to_string();
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
            Ok(ClientEvent::ServerMsg(msg)) => match msg {
                ServerMsg::Output(data) => {
                    parser.process(&data);
                    if !scrollback_mode {
                        scroll_offset = 0;
                    }
                    dirty = true;
                }
                ServerMsg::Resized { cols, rows } => {
                    parser.set_size(rows, cols);
                    dirty = true;
                }
                ServerMsg::Exited(code) => {
                    return Ok(code);
                }
            },
            Ok(ClientEvent::InputBytes(data)) => {
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
                                    let half = (current_inner_rows / 2) as usize;
                                    let max_scroll = parser.screen().scrollback();
                                    scroll_offset = (scroll_offset + half).min(max_scroll);
                                    dirty = true;
                                    i += 4;
                                    continue;
                                }
                                b'6' if i + 3 < data.len() && data[i + 3] == b'~' => {
                                    let half = (current_inner_rows / 2) as usize;
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
                                // Detach: just close socket, server keeps running
                                return Ok(0);
                            }
                            b'x' => {
                                // Stop: send Kill to server
                                let _ =
                                    protocol::write_client_msg(&mut sock_writer, &ClientMsg::Kill);
                                i += 1;
                                continue;
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
                                let _ = protocol::write_client_msg(
                                    &mut sock_writer,
                                    &ClientMsg::Input(vec![0x02]),
                                );
                            }
                            _ => {
                                // Not a recognized prefix command
                                let _ = protocol::write_client_msg(
                                    &mut sock_writer,
                                    &ClientMsg::Input(vec![0x02, b]),
                                );
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

                    // Normal input — find the next Ctrl+B and forward everything before it
                    let start = i;
                    while i < data.len() && data[i] != 0x02 {
                        i += 1;
                    }
                    let _ = protocol::write_client_msg(
                        &mut sock_writer,
                        &ClientMsg::Input(data[start..i].to_vec()),
                    );
                }
            }
            Ok(ClientEvent::ServerDisconnected) => {
                return Ok(0);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check for terminal resize
                if let Ok((cols, rows)) = get_term_size(tty_fd) {
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

    Ok(0)
}
