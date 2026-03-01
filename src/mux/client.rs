use anyhow::{Context, Result};
use ratatui::prelude::*;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::protocol::{self, ClientMsg, ServerMsg};
use super::terminal::{
    self, extract_selection_text, scrollback_line_count, write_osc52_clipboard, DrawFrameParams,
    InputAction, InputState, ScrollState,
};
use crate::session;

pub enum ClientResult {
    /// Session process exited (may fall through to another session)
    Exit(i32),
    /// User explicitly quit (Ctrl+P,Q or close button) — always exit box
    Quit,
    /// User requested switch to another session (name, sidebar state to restore)
    SwitchSession(String, Option<SidebarState>),
    /// User requested creating a new session with the given command
    NewSession(String),
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum SidebarEntryKind {
    WorkspaceHeader,
    Session,
}

pub(super) struct SidebarState {
    entries: Vec<SidebarEntry>,
    pub(super) selected: usize,
    /// Input buffer for new session command (Some = input mode active)
    pub(super) new_session_input: Option<String>,
    /// When true, keyboard input is routed to the sidebar for navigation
    pub(super) focused: bool,
}

pub(super) struct SidebarEntry {
    pub(super) kind: SidebarEntryKind,
    /// Display name (workspace name for headers, session name for sessions)
    pub(super) display: String,
    /// Full session name (workspace/session) — empty for headers
    pub(super) full_name: String,
    running: bool,
    local: bool,
}

enum ClientEvent {
    ServerMsg(ServerMsg),
    InputBytes(Vec<u8>),
    ServerDisconnected,
}

/// Build the sidebar session list with workspace grouping.
/// Returns entries and the index of the current session.
fn build_sidebar_entries(current_session: &str) -> (Vec<SidebarEntry>, usize) {
    let sessions = session::list().unwrap_or_default();
    let mut entries: Vec<SidebarEntry> = Vec::new();
    let mut current_ws = String::new();
    let mut selected = 0usize;

    for s in &sessions {
        let ws = session::workspace_name(&s.name);
        let sess_part = session::parse_name(&s.name).1;
        if ws != current_ws {
            current_ws = ws.to_string();
            entries.push(SidebarEntry {
                kind: SidebarEntryKind::WorkspaceHeader,
                display: ws.to_string(),
                full_name: String::new(),
                running: false,
                local: false,
            });
        }
        let running = if s.local {
            session::is_local_running(&s.name)
        } else {
            false
        };
        entries.push(SidebarEntry {
            kind: SidebarEntryKind::Session,
            display: sess_part.to_string(),
            full_name: s.name.clone(),
            running,
            local: s.local,
        });
    }

    // If no sessions found, add current as fallback
    if entries.is_empty() {
        let ws = session::workspace_name(current_session);
        let sess_part = session::parse_name(current_session).1;
        entries.push(SidebarEntry {
            kind: SidebarEntryKind::WorkspaceHeader,
            display: ws.to_string(),
            full_name: String::new(),
            running: false,
            local: false,
        });
        entries.push(SidebarEntry {
            kind: SidebarEntryKind::Session,
            display: sess_part.to_string(),
            full_name: current_session.to_string(),
            running: true,
            local: true,
        });
    }

    // Find the current session's index
    for (i, e) in entries.iter().enumerate() {
        if e.kind == SidebarEntryKind::Session && e.full_name == current_session {
            selected = i;
            break;
        }
    }

    (entries, selected)
}

/// Calculate sidebar width from entries.
fn sidebar_width(entries: &[SidebarEntry]) -> u16 {
    let max_name = entries
        .iter()
        .map(|e| match e.kind {
            SidebarEntryKind::WorkspaceHeader => e.display.len() + 1, // " ws"
            SidebarEntryKind::Session => e.display.len() + 3,         // "   name"
        })
        .max()
        .unwrap_or(8);
    let w = (max_name + 2).clamp(20, 40);
    w as u16
}

/// Draw the sidebar as a full-height left panel with grouped workspace headers.
fn draw_sidebar(
    f: &mut ratatui::Frame,
    sidebar: &SidebarState,
    area: Rect,
    _current_session: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let buf = f.buffer_mut();
    let focused = sidebar.focused;
    let bg_style = if focused {
        Style::default().bg(Color::Black).fg(Color::White)
    } else {
        Style::default().bg(Color::Black).fg(Color::DarkGray)
    };

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if x < buf.area().width && y < buf.area().height {
                let cell = &mut buf[(x, y)];
                cell.set_symbol(" ");
                cell.set_style(bg_style);
            }
        }
    }

    // Draw entries
    let content_width = area.width.saturating_sub(1); // reserve 1 for border
    for (idx, entry) in sidebar.entries.iter().enumerate() {
        let row_y = area.y + idx as u16;
        if row_y >= area.y + area.height {
            break;
        }

        let is_selected = idx == sidebar.selected;
        let (line, style) = match entry.kind {
            SidebarEntryKind::WorkspaceHeader => {
                let line = format!(" {}", entry.display);
                let style = Style::default().bg(Color::Black).fg(Color::Indexed(238));
                (line, style)
            }
            SidebarEntryKind::Session => {
                let line = format!("   {}", entry.display);
                let style = if is_selected {
                    if focused {
                        Style::default().bg(Color::White).fg(Color::Black)
                    } else {
                        Style::default().bg(Color::Indexed(238)).fg(Color::White)
                    }
                } else {
                    Style::default().bg(Color::Black).fg(Color::White)
                };
                (line, style)
            }
        };

        // Fill entire row with background first
        for x in area.x..area.x + content_width {
            if x < buf.area().width && row_y < buf.area().height {
                let cell = &mut buf[(x, row_y)];
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
        // Write the text
        for (col, ch) in line.chars().enumerate() {
            let x = area.x + col as u16;
            if x >= area.x + content_width {
                break;
            }
            if x < buf.area().width && row_y < buf.area().height {
                let cell = &mut buf[(x, row_y)];
                cell.set_symbol(&ch.to_string());
                cell.set_style(style);
            }
        }
    }

    // Draw new session input at the bottom if active
    if let Some(ref input) = sidebar.new_session_input {
        let row_y = area.y + area.height - 1;
        if row_y < buf.area().height {
            let input_style = Style::default().bg(Color::DarkGray).fg(Color::White);
            // Fill the row
            for x in area.x..area.x + content_width {
                if x < buf.area().width {
                    let cell = &mut buf[(x, row_y)];
                    cell.set_symbol(" ");
                    cell.set_style(input_style);
                }
            }
            let prompt = format!(" $ {}", input);
            for (col, ch) in prompt.chars().enumerate() {
                let x = area.x + col as u16;
                if x >= area.x + content_width {
                    break;
                }
                if x < buf.area().width {
                    let cell = &mut buf[(x, row_y)];
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(input_style);
                }
            }
        }
    }

    // Right border
    let border_x = area.x + area.width - 1;
    if border_x < buf.area().width {
        let border_style = Style::default().bg(Color::Black).fg(Color::DarkGray);
        for y in area.y..area.y + area.height {
            if y < buf.area().height {
                let cell = &mut buf[(border_x, y)];
                cell.set_symbol("\u{2502}"); // │
                cell.set_style(border_style);
            }
        }
    }
}

/// Process raw input bytes when the sidebar is open.
/// Returns Some(action) if the sidebar produces a result, None to keep it open.
enum SidebarAction {
    /// Switch to another session. `keep_sidebar` = true keeps sidebar open (keyboard nav).
    Switch {
        name: String,
        keep_sidebar: bool,
    },
    /// Create a new session with the given command
    NewSession(String),
    /// Return focus to the main pane
    Unfocus,
    Redraw,
    None,
}

/// Move selection to the next Session entry (skip headers), wrapping around.
fn sidebar_move_down(sidebar: &mut SidebarState) -> bool {
    let len = sidebar.entries.len();
    for offset in 1..len {
        let idx = (sidebar.selected + offset) % len;
        if sidebar.entries[idx].kind == SidebarEntryKind::Session {
            sidebar.selected = idx;
            return true;
        }
    }
    false
}

/// Move selection to the previous Session entry (skip headers), wrapping around.
fn sidebar_move_up(sidebar: &mut SidebarState) -> bool {
    let len = sidebar.entries.len();
    for offset in 1..len {
        let idx = (sidebar.selected + len - offset) % len;
        if sidebar.entries[idx].kind == SidebarEntryKind::Session {
            sidebar.selected = idx;
            return true;
        }
    }
    false
}

fn process_sidebar_input(
    data: &[u8],
    sidebar: &mut SidebarState,
    current_session: &str,
    sb_width: u16,
) -> SidebarAction {
    // Handle new session input mode
    if let Some(ref mut input) = sidebar.new_session_input {
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            match b {
                // ESC → cancel input
                0x1b => {
                    sidebar.new_session_input = None;
                    return SidebarAction::Redraw;
                }
                // Enter → submit
                b'\r' | b'\n' => {
                    let cmd = input.clone();
                    sidebar.new_session_input = None;
                    if cmd.is_empty() {
                        return SidebarAction::Redraw;
                    }
                    return SidebarAction::NewSession(cmd);
                }
                // Backspace
                0x7f | 0x08 => {
                    input.pop();
                    return SidebarAction::Redraw;
                }
                // Ctrl+U → clear input
                0x15 => {
                    input.clear();
                    return SidebarAction::Redraw;
                }
                // Printable ASCII
                0x20..=0x7e => {
                    input.push(b as char);
                    return SidebarAction::Redraw;
                }
                _ => {}
            }
            i += 1;
        }
        return SidebarAction::None;
    }

    let mut i = 0;
    let mut result = SidebarAction::None;
    while i < data.len() {
        let b = data[i];

        if b == 0x1b {
            // Check if it's a CSI sequence (arrow keys)
            if i + 2 < data.len() && data[i + 1] == b'[' {
                match data[i + 2] {
                    b'A' => {
                        // Up arrow — move selection
                        sidebar_move_up(sidebar);
                        result = SidebarAction::Redraw;
                        i += 3;
                        continue;
                    }
                    b'B' => {
                        // Down arrow — move selection
                        sidebar_move_down(sidebar);
                        result = SidebarAction::Redraw;
                        i += 3;
                        continue;
                    }
                    // SGR mouse: \x1b[<...
                    b'<' => {
                        if let Some((action, consumed)) =
                            parse_sidebar_mouse(data, i, sidebar, sb_width, current_session)
                        {
                            i += consumed;
                            match action {
                                SidebarAction::None => continue,
                                other => return other,
                            }
                        }
                        i += 3;
                        continue;
                    }
                    _ => {
                        // Skip unknown CSI
                        i += 3;
                        continue;
                    }
                }
            }
            // Bare ESC — unfocus sidebar, return to main pane
            sidebar.focused = false;
            return SidebarAction::Unfocus;
        }
        // j → move down
        if b == b'j' {
            sidebar_move_down(sidebar);
            result = SidebarAction::Redraw;
            i += 1;
            continue;
        }
        // k → move up
        if b == b'k' {
            sidebar_move_up(sidebar);
            result = SidebarAction::Redraw;
            i += 1;
            continue;
        }
        // Enter → switch to selected session and unfocus
        if b == b'\r' || b == b'\n' {
            let entry = &sidebar.entries[sidebar.selected];
            if entry.kind == SidebarEntryKind::Session
                && entry.full_name != current_session
                && (entry.running || entry.local)
            {
                sidebar.focused = false;
                return SidebarAction::Switch {
                    name: entry.full_name.clone(),
                    keep_sidebar: true,
                };
            }
            // If current session or not switchable, just unfocus
            sidebar.focused = false;
            return SidebarAction::Unfocus;
        }
        i += 1;
    }
    result
}

/// Parse SGR mouse event within sidebar context.
fn parse_sidebar_mouse(
    data: &[u8],
    i: usize,
    sidebar: &mut SidebarState,
    sb_width: u16,
    current_session: &str,
) -> Option<(SidebarAction, usize)> {
    // Format: \x1b[<Btn;Col;RowM or \x1b[<Btn;Col;Rowm
    let mut j = i + 3; // skip \x1b[<
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
                if param_idx != 2 {
                    return None;
                }
                let button = params[0];
                let col = params[1] as u16; // 1-indexed
                let row = params[2] as u16; // 1-indexed
                let pressed = data[j] == b'M';
                let consumed = j + 1 - i;

                // Left click on a session entry row
                if button == 0 && pressed && col <= sb_width && row >= 1 {
                    let entry_idx = (row - 1) as usize;
                    if entry_idx < sidebar.entries.len() {
                        let entry = &sidebar.entries[entry_idx];
                        // Skip workspace headers
                        if entry.kind == SidebarEntryKind::WorkspaceHeader {
                            return Some((SidebarAction::None, consumed));
                        }
                        if entry.full_name == current_session {
                            return Some((SidebarAction::None, consumed));
                        }
                        if !entry.running && !entry.local {
                            return Some((SidebarAction::None, consumed));
                        }
                        sidebar.selected = entry_idx;
                        return Some((
                            SidebarAction::Switch {
                                name: entry.full_name.clone(),
                                keep_sidebar: false,
                            },
                            consumed,
                        ));
                    }
                }

                // Consume other mouse events
                return Some((SidebarAction::None, consumed));
            }
            _ => return None,
        }
        j += 1;
    }
    None
}

pub fn run(
    session_name: &str,
    socket_path: &Path,
    tty_fd: i32,
    initial_sidebar: Option<SidebarState>,
) -> Result<ClientResult> {
    let (term_cols, term_rows) = terminal::get_term_size(tty_fd)?;

    let inner_rows = term_rows.saturating_sub(1);
    if inner_rows == 0 || term_cols == 0 {
        anyhow::bail!("Terminal too small");
    }

    // Build sidebar early so we know its width for the initial resize.
    let mut sidebar: SidebarState = initial_sidebar.unwrap_or_else(|| {
        let (entries, selected) = build_sidebar_entries(session_name);
        SidebarState {
            entries,
            selected,
            new_session_input: None,
            focused: false,
        }
    });
    let sb_w = sidebar_width(&sidebar.entries);
    let content_cols = term_cols.saturating_sub(sb_w);

    // Connect to server
    let sock = UnixStream::connect(socket_path).context("Failed to connect to mux server")?;
    let mut sock_writer = sock.try_clone().context("Failed to clone socket")?;

    // Set a write timeout so the client event loop doesn't block
    // indefinitely if the server is slow to read.
    let _ = sock_writer.set_write_timeout(Some(Duration::from_secs(5)));

    // Send initial Resize to server (subtract sidebar width)
    protocol::write_client_msg(
        &mut sock_writer,
        &ClientMsg::Resize {
            cols: content_cols,
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
        Ok(ServerMsg::Exited(code)) => return Ok(ClientResult::Exit(code)),
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
        Ok(ServerMsg::Exited(code)) => return Ok(ClientResult::Exit(code)),
        Ok(_) => {}
        Err(_) => return Ok(ClientResult::Exit(1)),
    }

    // Clear timeout for normal operation (reader thread handles its own blocking)
    sock_reader.set_read_timeout(None)?;

    // Create ratatui terminal.  Both internal buffers start empty, so the
    // first draw() will output every cell as a full diff — no clear() needed.
    let mut terminal = terminal::create_terminal(tty_fd, term_cols, term_rows)?;

    let display_name = super::display_name_for_session(session_name);
    let project_name = super::project_name_for_session(session_name);
    let header_color = super::color_for_session(session_name);
    let prefix_key = crate::config::load_mux_prefix_key();
    let mut input_state = InputState::new(prefix_key);

    // Draw the first frame immediately so the user sees content right
    // after a session switch instead of a blank screen.
    terminal::set_mouse_tracking(tty_fd, true);
    let mut mouse_tracking_on = true;

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

    // Input reader thread.
    // We dup the tty fd and close it on session switch to unblock the
    // thread's read() call so it exits cleanly.
    let tx_input = tx.clone();
    let tty_input_fd = unsafe { libc::dup(tty_fd) };
    if tty_input_fd < 0 {
        anyhow::bail!("Failed to dup tty fd for input");
    }
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(
                    tty_input_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break;
            }
            if tx_input
                .send(ClientEvent::InputBytes(buf[..n as usize].to_vec()))
                .is_err()
            {
                break;
            }
        }
        // Thread doesn't own the fd — main thread closes it.
    });

    let mut dirty = true;
    // Deferred session switch — set by sidebar Switch action so we repaint
    // (showing the updated selection highlight) before actually switching.
    let mut pending_switch: Option<(String, bool)> = None;

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
                    let (_, cur_cols) = parser.screen().size();
                    let cols_changed = cols != cur_cols;
                    parser.set_size(rows, cols);
                    if cols_changed {
                        parser.process(b"\x1b[H\x1b[2J");
                    }
                    input_state.scroll_offset = 0;
                    dirty = true;
                }
                ServerMsg::Exited(code) => {
                    return Ok(ClientResult::Exit(code));
                }
            },
            Ok(ClientEvent::InputBytes(data)) => {
                // Sidebar always handles mouse events in its area
                let sb_width = sidebar_width(&sidebar.entries);

                // Check if input should go to sidebar (focused, new_session_input,
                // or mouse in sidebar area)
                if sidebar.focused || sidebar.new_session_input.is_some() {
                    match process_sidebar_input(&data, &mut sidebar, session_name, sb_width) {
                        SidebarAction::Switch {
                            name: next,
                            keep_sidebar,
                        } => {
                            pending_switch = Some((next, keep_sidebar));
                            dirty = true;
                        }
                        SidebarAction::NewSession(cmd) => {
                            unsafe { libc::close(tty_input_fd) };
                            return Ok(ClientResult::NewSession(cmd));
                        }
                        SidebarAction::Unfocus | SidebarAction::Redraw => {
                            dirty = true;
                        }
                        SidebarAction::None => {}
                    }
                    continue;
                }

                // Check if it's a mouse event in the sidebar area
                let is_sidebar_mouse =
                    data.len() >= 3 && data[0] == 0x1b && data[1] == b'[' && data[2] == b'<' && {
                        // Quick-parse just the column to see if it's in sidebar
                        let mut j = 3usize;
                        let mut params = [0u32; 3];
                        let mut pi = 0;
                        let mut in_sidebar = false;
                        while j < data.len() {
                            match data[j] {
                                b'0'..=b'9' => {
                                    params[pi] =
                                        params[pi].saturating_mul(10) + (data[j] - b'0') as u32;
                                }
                                b';' => {
                                    pi += 1;
                                    if pi >= 3 {
                                        break;
                                    }
                                }
                                b'M' | b'm' => {
                                    if pi == 2 {
                                        in_sidebar = (params[1] as u16) <= sb_width;
                                    }
                                    break;
                                }
                                _ => break,
                            }
                            j += 1;
                        }
                        in_sidebar
                    };

                if is_sidebar_mouse {
                    match process_sidebar_input(&data, &mut sidebar, session_name, sb_width) {
                        SidebarAction::Switch {
                            name: next,
                            keep_sidebar,
                        } => {
                            pending_switch = Some((next, keep_sidebar));
                            dirty = true;
                        }
                        SidebarAction::NewSession(cmd) => {
                            unsafe { libc::close(tty_input_fd) };
                            return Ok(ClientResult::NewSession(cmd));
                        }
                        SidebarAction::Unfocus | SidebarAction::Redraw => {
                            dirty = true;
                        }
                        SidebarAction::None => {}
                    }
                    continue;
                }

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
                            return Ok(ClientResult::Quit);
                        }
                        InputAction::Kill => {
                            let _ = protocol::write_client_msg(&mut sock_writer, &ClientMsg::Kill);
                        }
                        InputAction::Redraw => {
                            dirty = true;
                        }
                        InputAction::FocusSidebar => {
                            // Refresh the list and focus the sidebar
                            input_state.selection = None;
                            input_state.drag_start = None;
                            let (entries, selected) = build_sidebar_entries(session_name);
                            sidebar = SidebarState {
                                entries,
                                selected,
                                new_session_input: None,
                                focused: true,
                            };
                            dirty = true;
                        }
                        InputAction::NewSession => {
                            sidebar.new_session_input = Some(String::new());
                            dirty = true;
                        }
                        InputAction::CopyToClipboard => {
                            if let Some(ref sel) = input_state.selection {
                                parser.set_scrollback(input_state.scroll_offset);
                                let text = extract_selection_text(parser.screen(), sel);
                                parser.set_scrollback(0);
                                if !text.is_empty() {
                                    write_osc52_clipboard(tty_fd, &text);
                                }
                            }
                        }
                    }
                }
            }
            Ok(ClientEvent::ServerDisconnected) => {
                return Ok(ClientResult::Exit(0));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Flush any buffered incomplete escape sequence
                if sidebar.new_session_input.is_none() {
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
                }

                // Check for terminal resize
                if let Ok((cols, rows)) = terminal::get_term_size(tty_fd) {
                    if cols != last_cols || rows != last_rows {
                        let cols_changed = cols != last_cols;
                        last_cols = cols;
                        last_rows = rows;
                        let new_inner = rows.saturating_sub(1);
                        let sb_w = sidebar_width(&sidebar.entries);
                        let content_cols = cols.saturating_sub(sb_w);
                        if new_inner > 0 && content_cols > 0 {
                            current_inner_rows = new_inner;
                            let _ = protocol::write_client_msg(
                                &mut sock_writer,
                                &ClientMsg::Resize {
                                    cols: content_cols,
                                    rows: new_inner,
                                },
                            );
                            parser.set_size(new_inner, content_cols);
                            if cols_changed {
                                parser.process(b"\x1b[H\x1b[2J");
                            }
                            terminal = terminal::create_terminal(tty_fd, cols, rows)?;
                            terminal.clear()?;
                        }
                        input_state.scroll_offset = 0;
                        input_state.selection = None;
                        input_state.drag_start = None;
                        dirty = true;
                    }
                }

                if dirty {
                    let max_scrollback = scrollback_line_count(&mut parser);

                    if !mouse_tracking_on {
                        mouse_tracking_on = true;
                        terminal::set_mouse_tracking(tty_fd, true);
                    }

                    parser.set_scrollback(input_state.scroll_offset);
                    let screen = parser.screen();
                    let scroll = ScrollState {
                        offset: input_state.scroll_offset,
                        max: max_scrollback,
                    };
                    let params = DrawFrameParams {
                        screen,
                        session_name,
                        project_name: &project_name,
                        scroll: &scroll,
                        command_mode: input_state.command_mode,
                        hover_close: input_state.hover_close,
                        header_color,
                        selection: input_state.selection.as_ref(),
                    };
                    // Write BSU/ESU through the same BufWriter as the
                    // frame data so the terminal emulator receives them
                    // as one contiguous byte stream (avoids the render
                    // being deferred when BSU/ESU travel on the raw
                    // tty_fd while frame data goes through the dup'd fd).
                    {
                        use std::io::Write;
                        let _ = terminal.backend_mut().write_all(b"\x1b[?2026h");
                    }
                    let sb_w = sidebar_width(&sidebar.entries);
                    terminal
                        .draw(|f| {
                            let full = f.area();
                            let sb_width = sb_w.min(full.width);
                            let right_width = full.width.saturating_sub(sb_width);
                            let sb_area = Rect {
                                x: full.x,
                                y: full.y,
                                width: sb_width,
                                height: full.height,
                            };
                            let right_area = Rect {
                                x: full.x + sb_width,
                                y: full.y,
                                width: right_width,
                                height: full.height,
                            };
                            draw_sidebar(f, &sidebar, sb_area, session_name);
                            terminal::draw_frame(f, &params, right_area);
                        })
                        .context("Failed to draw terminal frame")?;
                    {
                        use std::io::Write;
                        let _ = terminal.backend_mut().write_all(b"\x1b[?2026l");
                        let _ = std::io::Write::flush(terminal.backend_mut());
                    }
                    parser.set_scrollback(0);
                    dirty = false;

                    // Process deferred switch after repaint so the user
                    // sees the updated selection highlight.
                    if let Some((next, _keep_sidebar)) = pending_switch.take() {
                        unsafe { libc::close(tty_input_fd) };
                        // Always pass sidebar state to the next session
                        return Ok(ClientResult::SwitchSession(next, Some(sidebar)));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(ClientResult::Exit(0))
}
