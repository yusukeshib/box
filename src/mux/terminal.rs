use anyhow::{Context, Result};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};

/// RAII guard that restores terminal state on drop (including panics).
/// Uses /dev/tty so cleanup works even when stdin/stdout are redirected.
pub struct RawModeGuard {
    tty_fd: i32,
    orig_termios: libc::termios,
}

impl RawModeGuard {
    pub fn activate(tty: &mut std::fs::File) -> Result<Self> {
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

        // Enter alternate screen, hide cursor, enable SGR mouse tracking.
        // ?1000h = normal mouse tracking (reports button press/release)
        // ?1006h = SGR encoding (\x1b[<Btn;Col;RowM/m) for reliable parsing
        // We intercept scroll wheel (buttons 64/65) for scrollback and
        // forward other mouse events to the PTY child.
        // Text selection: hold Option (macOS) or Shift to bypass mouse tracking.
        tty.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h")?;
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
            // Disable mouse tracking, show cursor, leave alternate screen, reset attributes
            let _ = tty.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
        }
        unsafe {
            libc::tcsetattr(self.tty_fd, libc::TCSANOW, &self.orig_termios);
        }
    }
}

/// Custom ratatui Widget that renders a vt100::Screen.
/// The screen's scrollback offset must be set before rendering via
/// `parser.set_scrollback(offset)`, so `screen.cell()` returns the right cells.
pub struct TerminalWidget<'a> {
    pub screen: &'a vt100::Screen,
    pub show_cursor: bool,
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
pub fn get_term_size(fd: i32) -> Result<(u16, u16)> {
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
pub fn set_pty_size(pty: &pty_process::blocking::Pty, rows: u16, cols: u16) -> Result<()> {
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

/// Create a ratatui Terminal backed by a dup'd tty fd with a Fixed viewport.
///
/// Unlike `Terminal::resize()` (which sends a clear-screen escape), this
/// creates fresh internal buffers without writing anything to the terminal,
/// so the next `draw()` does a full diff-write with zero flicker.
pub fn create_terminal(
    tty_fd: i32,
    cols: u16,
    rows: u16,
) -> Result<Terminal<CrosstermBackend<std::fs::File>>> {
    let fd = unsafe { libc::dup(tty_fd) };
    if fd < 0 {
        anyhow::bail!("Failed to dup tty fd: {}", io::Error::last_os_error());
    }
    let writer = unsafe { std::fs::File::from_raw_fd(fd) };
    let backend = CrosstermBackend::new(writer);
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, cols, rows)),
        },
    )
    .context("Failed to create terminal")
}

/// Write bytes to PTY using raw libc::write to avoid panic-safety issues
/// with File::from_raw_fd + mem::forget.
pub fn write_bytes_to_pty(pty: &pty_process::blocking::Pty, data: &[u8]) -> Result<()> {
    let fd = pty.as_raw_fd();
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe {
            libc::write(
                fd,
                data[offset..].as_ptr() as *const libc::c_void,
                data.len() - offset,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error().into());
        }
        offset += n as usize;
    }
    Ok(())
}

/// Render the mux frame: header bar + terminal grid.
pub fn draw_frame(
    f: &mut ratatui::Frame,
    screen: &vt100::Screen,
    session_name: &str,
    project_name: &str,
    show_help: bool,
    is_scrollback: bool,
) {
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
    let left = if project_name.is_empty() {
        format!(" {} ", session_name)
    } else {
        format!(" {} > {} ", project_name, session_name)
    };
    let right = if show_help {
        " Ctrl+P,Q:detach  ,X:stop  ,[:scroll  ,?:help "
    } else if is_scrollback {
        " SCROLL: Up/Down PgUp/PgDn  q:exit "
    } else {
        " Ctrl+P,? for help "
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
}

/// Input processing state machine for the Ctrl+P prefix and scrollback mode.
pub struct InputState {
    pub prefix_active: bool,
    pub scroll_offset: usize,
    pub scrollback_mode: bool,
    pub show_help: bool,
    /// Bytes from an incomplete escape sequence carried over from the
    /// previous read.  Combined with the next input in `process()`.
    pending: Vec<u8>,
}

pub enum InputAction {
    /// Forward bytes to the PTY / server
    Forward(Vec<u8>),
    /// Detach from the session
    Detach,
    /// Kill the child / send Kill to server
    Kill,
    /// Screen needs redraw
    Redraw,
}

/// Try to parse an SGR mouse sequence at data[i..].
/// Format: \x1b[<Btn;Col;RowM (press) or \x1b[<Btn;Col;Rowm (release).
/// Returns (button, bytes_consumed) on success, None if not a valid sequence.
fn parse_sgr_mouse(data: &[u8], i: usize) -> Option<(u32, usize)> {
    // Need at least \x1b[< = 3 bytes
    if i + 2 >= data.len() || data[i] != 0x1b || data[i + 1] != b'[' || data[i + 2] != b'<' {
        return None;
    }
    let mut j = i + 3;
    let mut params = [0u32; 3];
    let mut param_idx = 0;
    while j < data.len() {
        match data[j] {
            b'0'..=b'9' => {
                params[param_idx] = params[param_idx].saturating_mul(10) + (data[j] - b'0') as u32;
            }
            b';' => {
                param_idx += 1;
                if param_idx >= 3 {
                    return None;
                }
            }
            b'M' | b'm' => {
                if param_idx == 2 {
                    return Some((params[0], j + 1 - i));
                }
                return None;
            }
            _ => return None,
        }
        j += 1;
    }
    None // incomplete sequence
}

impl InputState {
    pub fn new() -> Self {
        Self {
            prefix_active: false,
            scroll_offset: 0,
            scrollback_mode: false,
            show_help: false,
            pending: Vec::new(),
        }
    }

    /// Flush any buffered bytes that didn't form a complete escape
    /// sequence within the timeout window.  Called from the main loop's
    /// Timeout branch so a bare ESC isn't held indefinitely.
    pub fn flush_pending(
        &mut self,
        current_inner_rows: u16,
        max_scrollback: usize,
    ) -> Vec<InputAction> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let data = std::mem::take(&mut self.pending);
        // Process without buffering — treat whatever we have as final.
        self.process_inner(&data, current_inner_rows, max_scrollback, false)
    }

    /// Process raw input bytes, returning actions to perform.
    /// `current_inner_rows` is used for PgUp/PgDn scroll step.
    /// `max_scrollback` is from `parser.screen().scrollback()`.
    pub fn process(
        &mut self,
        new_data: &[u8],
        current_inner_rows: u16,
        max_scrollback: usize,
    ) -> Vec<InputAction> {
        // Combine any pending bytes from a previous incomplete sequence.
        let combined;
        let data: &[u8] = if self.pending.is_empty() {
            new_data
        } else {
            let mut buf = std::mem::take(&mut self.pending);
            buf.extend_from_slice(new_data);
            combined = buf;
            &combined
        };
        self.process_inner(data, current_inner_rows, max_scrollback, true)
    }

    fn process_inner(
        &mut self,
        data: &[u8],
        current_inner_rows: u16,
        max_scrollback: usize,
        allow_buffer: bool,
    ) -> Vec<InputAction> {
        let mut actions = Vec::new();
        let mut i = 0;

        while i < data.len() {
            let b = data[i];

            // Buffer incomplete escape sequences for the next call.
            // Terminal reads often split \x1b from the rest of a CSI
            // sequence (e.g. SGR mouse \x1b[<64;44;51M).
            if allow_buffer && b == 0x1b {
                let incomplete = if i + 1 >= data.len() {
                    true // lone ESC
                } else if data[i + 1] == b'[' {
                    // CSI — complete when a byte >= 0x40 appears after \x1b[
                    !data[i + 2..].iter().any(|&c| c >= 0x40)
                } else {
                    false // \x1b + non-'[' is a complete 2-byte sequence
                };
                if incomplete {
                    self.pending = data[i..].to_vec();
                    break;
                }
            }

            if self.scrollback_mode {
                // SGR mouse scroll in scrollback mode
                if let Some((btn, consumed)) = parse_sgr_mouse(data, i) {
                    match btn {
                        64 => {
                            if self.scroll_offset < max_scrollback {
                                self.scroll_offset += 3;
                                self.scroll_offset = self.scroll_offset.min(max_scrollback);
                            }
                            actions.push(InputAction::Redraw);
                        }
                        65 => {
                            self.scroll_offset = self.scroll_offset.saturating_sub(3);
                            if self.scroll_offset == 0 {
                                self.scrollback_mode = false;
                            }
                            actions.push(InputAction::Redraw);
                        }
                        _ => {} // ignore other mouse events in scrollback
                    }
                    i += consumed;
                    continue;
                }
                if b == 0x1b && i + 2 < data.len() && data[i + 1] == b'[' {
                    match data[i + 2] {
                        b'A' => {
                            if self.scroll_offset < max_scrollback {
                                self.scroll_offset += 1;
                            }
                            actions.push(InputAction::Redraw);
                            i += 3;
                            continue;
                        }
                        b'B' => {
                            self.scroll_offset = self.scroll_offset.saturating_sub(1);
                            if self.scroll_offset == 0 {
                                self.scrollback_mode = false;
                            }
                            actions.push(InputAction::Redraw);
                            i += 3;
                            continue;
                        }
                        b'5' if i + 3 < data.len() && data[i + 3] == b'~' => {
                            let half = (current_inner_rows / 2) as usize;
                            self.scroll_offset = (self.scroll_offset + half).min(max_scrollback);
                            actions.push(InputAction::Redraw);
                            i += 4;
                            continue;
                        }
                        b'6' if i + 3 < data.len() && data[i + 3] == b'~' => {
                            let half = (current_inner_rows / 2) as usize;
                            self.scroll_offset = self.scroll_offset.saturating_sub(half);
                            if self.scroll_offset == 0 {
                                self.scrollback_mode = false;
                            }
                            actions.push(InputAction::Redraw);
                            i += 4;
                            continue;
                        }
                        _ => {}
                    }
                }
                match b {
                    b'q' | 0x1b => {
                        if b == b'q' || (b == 0x1b && (i + 1 >= data.len() || data[i + 1] != 0x5b))
                        {
                            self.scrollback_mode = false;
                            self.scroll_offset = 0;
                            actions.push(InputAction::Redraw);
                            i += 1;
                            continue;
                        }
                    }
                    _ => {}
                }
                i += 1;
                continue;
            }

            if self.prefix_active {
                self.prefix_active = false;
                match b {
                    b'q' | b'Q' | 0x11 => {
                        // q, Q, or Ctrl+Q
                        actions.push(InputAction::Detach);
                        return actions;
                    }
                    b'x' | b'X' | 0x18 => {
                        // x, X, or Ctrl+X
                        actions.push(InputAction::Kill);
                        i += 1;
                        continue;
                    }
                    b'[' => {
                        self.scrollback_mode = true;
                        actions.push(InputAction::Redraw);
                        i += 1;
                        continue;
                    }
                    b'?' => {
                        self.show_help = !self.show_help;
                        actions.push(InputAction::Redraw);
                        i += 1;
                        continue;
                    }
                    0x10 => {
                        // Ctrl+P Ctrl+P -> send literal Ctrl+P
                        actions.push(InputAction::Forward(vec![0x10]));
                    }
                    _ => {
                        // Not a recognized prefix command — send Ctrl+P + the byte
                        actions.push(InputAction::Forward(vec![0x10, b]));
                    }
                }
                i += 1;
                continue;
            }

            if b == 0x10 {
                self.prefix_active = true;
                i += 1;
                continue;
            }

            // SGR mouse: intercept scroll wheel, forward other events to PTY.
            if let Some((btn, consumed)) = parse_sgr_mouse(data, i) {
                match btn {
                    64 if max_scrollback > 0 => {
                        // Scroll wheel up — enter scrollback mode
                        self.scrollback_mode = true;
                        self.scroll_offset = 3;
                        actions.push(InputAction::Redraw);
                    }
                    65 => {} // Scroll wheel down at bottom — ignore
                    _ => {
                        // Forward other mouse events (click, drag) to PTY
                        actions.push(InputAction::Forward(data[i..i + consumed].to_vec()));
                    }
                }
                i += consumed;
                continue;
            }

            // Fallback: auto-enter scrollback on Up arrow (?1007h alternate scroll).
            if b == 0x1b
                && i + 2 < data.len()
                && data[i + 1] == b'['
                && data[i + 2] == b'A'
                && max_scrollback > 0
            {
                self.scrollback_mode = true;
                self.scroll_offset = 1;
                actions.push(InputAction::Redraw);
                i += 3;
                continue;
            }

            // Normal input — find the next Ctrl+P (if any) and forward
            // everything before it in one write.  Also stop before any ESC
            // that could be a scroll-wheel Up sequence so the check above
            // can evaluate it on the next iteration.
            let start = i;
            while i < data.len() && data[i] != 0x10 && data[i] != 0x1b {
                i += 1;
            }
            // Forward ESC sequences that are NOT scroll-wheel up.
            if i == start && i < data.len() && data[i] == 0x1b {
                let seq_start = i;
                // Consume the ESC and any CSI sequence bytes
                i += 1;
                if i < data.len() && data[i] == b'[' {
                    i += 1;
                    // Skip parameter bytes (0x30-0x3F) and intermediate bytes (0x20-0x2F)
                    while i < data.len() && data[i] >= 0x20 && data[i] < 0x40 {
                        i += 1;
                    }
                    // Skip final byte
                    if i < data.len() {
                        i += 1;
                    }
                }
                actions.push(InputAction::Forward(data[seq_start..i].to_vec()));
            } else if i > start {
                actions.push(InputAction::Forward(data[start..i].to_vec()));
            }
        }

        actions
    }
}

/// Install a panic hook that restores terminal state.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = tty.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[?25h\x1b[?1049l\x1b[0m");
            let _ = tty.flush();
            use std::os::unix::io::FromRawFd;
            let _ = std::process::Command::new("stty")
                .arg("sane")
                .stdin(unsafe { std::process::Stdio::from_raw_fd(tty.as_raw_fd()) })
                .status();
        }
        default_hook(info);
    }));
}
