use anyhow::{Context, Result};
use ratatui::prelude::*;
use ratatui::widgets::Widget;
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

        // Enter alternate screen, hide cursor.
        // Mouse tracking is enabled dynamically when scrollback content
        // exists, so native text selection works when there's nothing to scroll.
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
            // Disable mouse tracking, show cursor, leave alternate screen, reset attributes
            let _ = tty.write_all(b"\x1b[?1006l\x1b[?1003l\x1b[?25h\x1b[?1049l\x1b[0m");
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
    pub selection: Option<&'a Selection>,
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

                let ch = cell.contents();
                let display_ch = if ch.is_empty() { " " } else { ch.as_str() };

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

                if let Some(sel) = self.selection {
                    if sel.contains(y as u16, x as u16) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                }

                let buf_cell = &mut buf[(buf_x, buf_y)];
                buf_cell.set_symbol(display_ch);
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
) -> Result<Terminal<CrosstermBackend<io::BufWriter<std::fs::File>>>> {
    let fd = unsafe { libc::dup(tty_fd) };
    if fd < 0 {
        anyhow::bail!("Failed to dup tty fd: {}", io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let writer = io::BufWriter::new(file);
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

/// Return the number of lines in the scrollback buffer.
///
/// `screen().scrollback()` returns the current *viewing offset*, not the
/// total number of buffered lines.  The only public way to discover the
/// actual count is to set the offset to MAX (which clamps to `len()`),
/// read the value, and restore.
pub fn scrollback_line_count(parser: &mut vt100::Parser) -> usize {
    parser.set_scrollback(usize::MAX);
    let count = parser.screen().scrollback();
    parser.set_scrollback(0);
    count
}

/// A rectangular text selection on the terminal grid (0-indexed, grid-relative).
#[derive(Clone, Debug)]
pub struct Selection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

impl Selection {
    /// Return (top_row, left_col, bottom_row, right_col) in reading order.
    pub fn ordered(&self) -> (u16, u16, u16, u16) {
        if (self.start_row, self.start_col) <= (self.end_row, self.end_col) {
            (self.start_row, self.start_col, self.end_row, self.end_col)
        } else {
            (self.end_row, self.end_col, self.start_row, self.start_col)
        }
    }

    /// Check if a (row, col) cell falls within the selection range.
    pub fn contains(&self, row: u16, col: u16) -> bool {
        let (top_row, top_col, bot_row, bot_col) = self.ordered();
        if row < top_row || row > bot_row {
            return false;
        }
        if row == top_row && row == bot_row {
            return col >= top_col && col <= bot_col;
        }
        if row == top_row {
            return col >= top_col;
        }
        if row == bot_row {
            return col <= bot_col;
        }
        true
    }
}

/// Scrollback state passed to `draw_frame` for rendering the scrollbar.
pub struct ScrollState {
    pub offset: usize,
    pub max: usize,
}

/// Parameters for rendering the mux frame.
pub struct DrawFrameParams<'a> {
    pub screen: &'a vt100::Screen,
    pub scroll: &'a ScrollState,
    pub selection: Option<&'a Selection>,
}

/// Render the mux frame: terminal grid (full area, no header).
/// `area` defines the region to draw into (allows callers to constrain it).
pub fn draw_frame(f: &mut ratatui::Frame, params: &DrawFrameParams, area: Rect) {
    let screen = params.screen;
    let scrolled_up = params.scroll.offset > 0;
    let scroll_offset = params.scroll.offset;
    let max_scrollback = params.scroll.max;

    let grid_area = area;

    let widget = TerminalWidget {
        screen,
        show_cursor: !scrolled_up,
        selection: params.selection,
    };
    f.render_widget(widget, grid_area);

    // Render scrollbar when there is scrollback content
    if max_scrollback > 0 && grid_area.height > 0 {
        let track_height = grid_area.height as usize;
        // Thumb size: at least 1 row, proportional to visible / total
        let total_lines = max_scrollback + track_height;
        let thumb_size = (track_height * track_height / total_lines).max(1);
        // Thumb position: 0 = bottom (scroll_offset 0), top = max scroll
        let max_thumb_top = track_height.saturating_sub(thumb_size);
        let thumb_top = if max_scrollback > 0 {
            scroll_offset * max_thumb_top / max_scrollback
        } else {
            0
        };
        // Invert: scroll_offset=max means thumb at top (y=0)
        let thumb_y_start = max_thumb_top - thumb_top;

        let scrollbar_x = grid_area.x + grid_area.width.saturating_sub(1);
        let track_style = Style::default().add_modifier(Modifier::DIM);
        let thumb_style = Style::default();

        for row in 0..track_height {
            let y = grid_area.y + row as u16;
            if scrollbar_x >= f.area().width || y >= f.area().height {
                continue;
            }
            let cell = &mut f.buffer_mut()[(scrollbar_x, y)];
            if row >= thumb_y_start && row < thumb_y_start + thumb_size {
                cell.set_symbol("\u{2588}"); // █ (full block)
                cell.set_style(thumb_style);
            } else {
                cell.set_symbol("\u{2502}"); // │ (thin vertical line)
                cell.set_style(track_style);
            }
        }
    }
}

/// Input processing state machine for COMMAND mode and scroll.
pub struct InputState {
    pub command_mode: bool,
    pub scroll_offset: usize,
    /// The control byte that enters COMMAND mode (default 0x10 = Ctrl+P).
    prefix_key: u8,
    /// True while the user is click-dragging the scrollbar thumb.
    dragging_scrollbar: bool,
    /// Bytes from an incomplete escape sequence carried over from the
    /// previous read.  Combined with the next input in `process()`.
    pending: Vec<u8>,
    /// Active text selection (mouse drag).
    pub selection: Option<Selection>,
    /// Grid cell where the left mouse button went down (row, col; 0-indexed).
    /// Used to distinguish a click from a drag.
    pub drag_start: Option<(u16, u16)>,
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
    /// Refresh the session switcher sidebar
    FocusSidebar,
    /// Create a new session in the same workspace
    NewSession,
    /// Copy the current selection to clipboard via OSC 52
    CopyToClipboard,
}

struct SgrMouseEvent {
    button: u32,
    col: u16,
    row: u16,
    pressed: bool,
}

/// Try to parse an SGR mouse sequence at data[i..].
/// Format: \x1b[<Btn;Col;RowM (press) or \x1b[<Btn;Col;Rowm (release).
/// Returns (event, bytes_consumed) on success, None if not a valid sequence.
fn parse_sgr_mouse(data: &[u8], i: usize) -> Option<(SgrMouseEvent, usize)> {
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
                    return Some((
                        SgrMouseEvent {
                            button: params[0],
                            col: params[1] as u16,
                            row: params[2] as u16,
                            pressed: data[j] == b'M',
                        },
                        j + 1 - i,
                    ));
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
    pub fn new(prefix_key: u8) -> Self {
        Self {
            command_mode: false,
            scroll_offset: 0,
            prefix_key,
            dragging_scrollbar: false,
            pending: Vec::new(),
            selection: None,
            drag_start: None,
        }
    }

    /// Flush any buffered bytes that didn't form a complete escape
    /// sequence within the timeout window.  Called from the main loop's
    /// Timeout branch so a bare ESC isn't held indefinitely.
    pub fn flush_pending(
        &mut self,
        current_inner_rows: u16,
        term_cols: u16,
        max_scrollback: usize,
        col_offset: u16,
    ) -> Vec<InputAction> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let data = std::mem::take(&mut self.pending);
        // Process without buffering — treat whatever we have as final.
        self.process_inner(
            &data,
            current_inner_rows,
            term_cols,
            max_scrollback,
            false,
            col_offset,
        )
    }

    /// Process raw input bytes, returning actions to perform.
    /// `current_inner_rows` is used for PgUp/PgDn scroll step.
    /// `term_cols` is the terminal width (for scrollbar click detection).
    /// `max_scrollback` is from `parser.screen().scrollback()`.
    /// `col_offset` is subtracted from mouse column coords (e.g. sidebar width).
    pub fn process(
        &mut self,
        new_data: &[u8],
        current_inner_rows: u16,
        term_cols: u16,
        max_scrollback: usize,
        col_offset: u16,
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
        self.process_inner(
            data,
            current_inner_rows,
            term_cols,
            max_scrollback,
            true,
            col_offset,
        )
    }

    fn process_inner(
        &mut self,
        data: &[u8],
        current_inner_rows: u16,
        term_cols: u16,
        max_scrollback: usize,
        allow_buffer: bool,
        col_offset: u16,
    ) -> Vec<InputAction> {
        let mut actions = Vec::new();
        let mut i = 0;

        while i < data.len() {
            let b = data[i];

            // Buffer incomplete escape sequences for the next call.
            // Terminal reads often split \x1b from the rest of a CSI
            // sequence (e.g. arrow key \x1b[A).
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

            // Gate 1: Always intercept SGR mouse events for scrollback / scrollbar
            if let Some((mut mouse, consumed)) = parse_sgr_mouse(data, i) {
                // Adjust column for sidebar offset (absolute → pane-relative)
                mouse.col = mouse.col.saturating_sub(col_offset);
                // Motion events (button 35 = motion with no button pressed)
                if mouse.button == 35 {
                    i += consumed;
                    continue;
                }
                // Left-drag: update selection (not scrollbar)
                if mouse.button == 32 && !self.dragging_scrollbar {
                    if let Some((start_row, start_col)) = self.drag_start {
                        // Convert SGR 1-indexed coords to 0-indexed grid coords
                        let grid_row = mouse.row.saturating_sub(1);
                        let grid_col = mouse.col.saturating_sub(1);
                        if grid_row != start_row || grid_col != start_col {
                            self.selection = Some(Selection {
                                start_row,
                                start_col,
                                end_row: grid_row,
                                end_col: grid_col,
                            });
                            actions.push(InputAction::Redraw);
                        }
                    }
                    i += consumed;
                    continue;
                }

                match mouse.button {
                    64 => {
                        // Scroll wheel up — clear selection
                        self.selection = None;
                        self.drag_start = None;
                        self.scroll_offset = (self.scroll_offset + 3).min(max_scrollback);
                        actions.push(InputAction::Redraw);
                    }
                    65 => {
                        // Scroll wheel down — clear selection
                        self.selection = None;
                        self.drag_start = None;
                        self.scroll_offset = self.scroll_offset.saturating_sub(3);
                        actions.push(InputAction::Redraw);
                    }
                    // Left click on scrollbar column (SGR coords are 1-indexed)
                    0 if mouse.pressed
                        && max_scrollback > 0
                        && mouse.col == term_cols
                        && mouse.row >= 1 =>
                    {
                        let grid_row = (mouse.row - 1) as usize;
                        let track_height = current_inner_rows as usize;
                        if grid_row < track_height && track_height > 1 {
                            self.scroll_offset =
                                max_scrollback * (track_height - 1 - grid_row) / (track_height - 1);
                            self.dragging_scrollbar = true;
                            actions.push(InputAction::Redraw);
                        }
                    }
                    // Left click on grid area → start drag
                    0 if mouse.pressed && mouse.row >= 1 => {
                        let had_selection = self.selection.is_some();
                        self.selection = None;
                        self.drag_start =
                            Some((mouse.row.saturating_sub(1), mouse.col.saturating_sub(1)));
                        if had_selection {
                            actions.push(InputAction::Redraw);
                        }
                    }
                    // Left button drag while scrollbar is held
                    32 if mouse.pressed && self.dragging_scrollbar && max_scrollback > 0 => {
                        let track_height = current_inner_rows as usize;
                        if track_height > 1 {
                            let grid_row =
                                (mouse.row.saturating_sub(1) as usize).min(track_height - 1);
                            self.scroll_offset =
                                max_scrollback * (track_height - 1 - grid_row) / (track_height - 1);
                            actions.push(InputAction::Redraw);
                        }
                    }
                    // Left button release — stop drag, copy selection
                    0 if !mouse.pressed => {
                        self.dragging_scrollbar = false;
                        if self.selection.is_some() {
                            actions.push(InputAction::CopyToClipboard);
                        }
                        self.drag_start = None;
                    }
                    _ => {} // consume other mouse events
                }
                i += consumed;
                continue;
            }

            // Gate 2: COMMAND mode — intercept keys directly
            if self.command_mode {
                // Bare ESC (not part of a CSI sequence) exits COMMAND mode
                if b == 0x1b && (i + 1 >= data.len() || data[i + 1] != b'[') {
                    self.command_mode = false;
                    self.scroll_offset = 0;
                    actions.push(InputAction::Redraw);
                    i += 1;
                    continue;
                }
                // Ctrl+Q — detach
                if b == 0x11 {
                    actions.push(InputAction::Detach);
                    return actions;
                }
                // 'a' or Ctrl+A — refresh session sidebar
                if b == b'a' || b == 0x01 {
                    self.command_mode = false;
                    actions.push(InputAction::FocusSidebar);
                    i += 1;
                    continue;
                }
                // 'n' — create new session in same workspace
                if b == b'n' {
                    self.command_mode = false;
                    actions.push(InputAction::NewSession);
                    i += 1;
                    continue;
                }
                // Ctrl+X — kill
                if b == 0x18 {
                    actions.push(InputAction::Kill);
                    i += 1;
                    continue;
                }
                // Ctrl+P — scroll up 1 line
                if b == 0x10 {
                    self.scroll_offset = (self.scroll_offset + 1).min(max_scrollback);
                    actions.push(InputAction::Redraw);
                    i += 1;
                    continue;
                }
                // Ctrl+N — scroll down 1 line
                if b == 0x0E {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    actions.push(InputAction::Redraw);
                    i += 1;
                    continue;
                }
                // Ctrl+U — half page up
                if b == 0x15 {
                    let half = (current_inner_rows / 2) as usize;
                    self.scroll_offset = (self.scroll_offset + half).min(max_scrollback);
                    actions.push(InputAction::Redraw);
                    i += 1;
                    continue;
                }
                // Ctrl+D — half page down
                if b == 0x04 {
                    let half = (current_inner_rows / 2) as usize;
                    self.scroll_offset = self.scroll_offset.saturating_sub(half);
                    actions.push(InputAction::Redraw);
                    i += 1;
                    continue;
                }
                // Arrow keys, PgUp/PgDn
                if b == 0x1b && i + 2 < data.len() && data[i + 1] == b'[' {
                    match data[i + 2] {
                        b'A' => {
                            // Up arrow
                            self.scroll_offset = (self.scroll_offset + 1).min(max_scrollback);
                            actions.push(InputAction::Redraw);
                            i += 3;
                            continue;
                        }
                        b'B' => {
                            // Down arrow
                            self.scroll_offset = self.scroll_offset.saturating_sub(1);
                            actions.push(InputAction::Redraw);
                            i += 3;
                            continue;
                        }
                        b'5' if i + 3 < data.len() && data[i + 3] == b'~' => {
                            // PgUp
                            let half = (current_inner_rows / 2) as usize;
                            self.scroll_offset = (self.scroll_offset + half).min(max_scrollback);
                            actions.push(InputAction::Redraw);
                            i += 4;
                            continue;
                        }
                        b'6' if i + 3 < data.len() && data[i + 3] == b'~' => {
                            // PgDn
                            let half = (current_inner_rows / 2) as usize;
                            self.scroll_offset = self.scroll_offset.saturating_sub(half);
                            actions.push(InputAction::Redraw);
                            i += 4;
                            continue;
                        }
                        _ => {}
                    }
                }
                // Consume all other keys in COMMAND mode (don't forward to PTY)
                i += 1;
                continue;
            }

            // Gate 3: prefix key enters COMMAND mode
            if b == self.prefix_key {
                self.command_mode = true;
                // Clear selection on keyboard input
                self.selection = None;
                self.drag_start = None;
                actions.push(InputAction::Redraw);
                i += 1;
                continue;
            }

            // Clear selection on keyboard input (Gate 4)
            if self.selection.is_some() {
                self.selection = None;
                self.drag_start = None;
                actions.push(InputAction::Redraw);
            }

            // Gate 4: Normal input — batch forward until prefix_key or ESC
            let start = i;
            while i < data.len() && data[i] != self.prefix_key && data[i] != 0x1b {
                i += 1;
            }
            if i > start {
                actions.push(InputAction::Forward(data[start..i].to_vec()));
            } else if i < data.len() && data[i] == 0x1b {
                // ESC byte not consumed by any special handler above.
                // Forward it along with any CSI sequence that follows,
                // so escape sequences like \x1b[A aren't split across writes.
                let esc_start = i;
                i += 1;
                if i < data.len() && data[i] == b'[' {
                    i += 1; // skip '['
                    while i < data.len() && data[i] >= 0x20 && data[i] < 0x40 {
                        i += 1; // skip parameter/intermediate bytes
                    }
                    if i < data.len() && data[i] >= 0x40 {
                        i += 1; // include final byte
                    }
                }
                actions.push(InputAction::Forward(data[esc_start..i].to_vec()));
            }
        }

        actions
    }
}

/// Enable or disable SGR mouse tracking on the terminal.
/// Toggled dynamically based on whether scrollback content exists:
/// off when scrollback is empty (native text selection works),
/// on when there's content to scroll through.
/// Mode 1000 = basic press/release, 1002 = button-event (drag), 1006 = SGR encoding.
/// Write raw bytes to a tty fd (retries on short writes).
fn tty_write(tty_fd: i32, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe {
            libc::write(
                tty_fd,
                data[offset..].as_ptr() as *const libc::c_void,
                data.len() - offset,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue; // retry on EINTR
            }
            break;
        }
        if n == 0 {
            break;
        }
        offset += n as usize;
    }
}

/// Begin DEC synchronized update mode (DECSM 2026).
/// Tells the terminal to buffer all output until `end_sync_update` is called,
/// then render everything in a single atomic frame.
pub fn begin_sync_update(tty_fd: i32) {
    tty_write(tty_fd, b"\x1b[?2026h");
}

/// End DEC synchronized update mode (DECRM 2026).
pub fn end_sync_update(tty_fd: i32) {
    tty_write(tty_fd, b"\x1b[?2026l");
}

pub fn set_mouse_tracking(tty_fd: i32, enable: bool) {
    let seq: &[u8] = if enable {
        // Mode 1003: any-event tracking (reports motion even without buttons held)
        // Mode 1006: SGR extended mouse encoding
        b"\x1b[?1003h\x1b[?1006h"
    } else {
        b"\x1b[?1006l\x1b[?1003l"
    };
    tty_write(tty_fd, seq);
}

/// Write text to the system clipboard via the OSC 52 escape sequence.
/// The terminal emulator intercepts this and sets the clipboard contents.
pub fn write_osc52_clipboard(tty_fd: i32, text: &str) {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let src = text.as_bytes();
    let mut encoded = Vec::with_capacity(4 * (src.len() + 2) / 3);
    for chunk in src.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(B64[((n >> 18) & 0x3f) as usize]);
        encoded.push(B64[((n >> 12) & 0x3f) as usize]);
        if chunk.len() > 1 {
            encoded.push(B64[((n >> 6) & 0x3f) as usize]);
        } else {
            encoded.push(b'=');
        }
        if chunk.len() > 2 {
            encoded.push(B64[(n & 0x3f) as usize]);
        } else {
            encoded.push(b'=');
        }
    }
    // \x1b]52;c;<base64>\x07
    let mut seq = Vec::with_capacity(7 + encoded.len() + 1);
    seq.extend_from_slice(b"\x1b]52;c;");
    seq.extend_from_slice(&encoded);
    seq.push(0x07);
    tty_write(tty_fd, &seq);
}

/// Extract the text covered by `selection` from a vt100 screen.
/// Trims trailing whitespace per line and joins with newlines.
pub fn extract_selection_text(screen: &vt100::Screen, selection: &Selection) -> String {
    let (top_row, top_col, bot_row, bot_col) = selection.ordered();
    let cols = screen.size().1;
    let mut lines: Vec<String> = Vec::new();
    for row in top_row..=bot_row {
        let col_start = if row == top_row { top_col } else { 0 };
        let col_end = if row == bot_row { bot_col } else { cols - 1 };
        let mut line = String::new();
        for col in col_start..=col_end {
            if let Some(cell) = screen.cell(row, col) {
                let contents = cell.contents();
                if contents.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(contents.as_str());
                }
            } else {
                line.push(' ');
            }
        }
        lines.push(line.trim_end().to_string());
    }
    // Remove trailing empty lines
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Install a panic hook that restores terminal state.
/// Guarded by `Once` so multiple calls (e.g. standalone + client paths) are safe.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(install_panic_hook_inner);
}

fn install_panic_hook_inner() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = tty.write_all(b"\x1b[?1006l\x1b[?1003l\x1b[?25h\x1b[?1049l\x1b[0m");
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
