use anyhow::{Context, Result};
use ratatui::prelude::*;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::protocol::{self, ClientMsg, ServerMsg};
use super::terminal::{
    self, scrollback_line_count, DrawFrameParams, InputAction, InputState, ScrollState,
};
use crate::session;

pub enum ClientResult {
    /// Normal exit (detach or session exited)
    Exit(i32),
    /// User requested switch to another session
    SwitchSession(String),
}

struct SidebarState {
    sessions: Vec<SidebarEntry>,
    selected: usize,
}

struct SidebarEntry {
    name: String,
    running: bool,
    local: bool,
}

enum ClientEvent {
    ServerMsg(ServerMsg),
    InputBytes(Vec<u8>),
    ServerDisconnected,
}

/// Build the sidebar session list, returning entries and the index of the current session.
fn build_sidebar_entries(current_session: &str) -> (Vec<SidebarEntry>, usize) {
    let sessions = session::list().unwrap_or_default();
    let mut entries: Vec<SidebarEntry> = sessions
        .into_iter()
        .map(|s| {
            let running = if s.local {
                session::is_local_running(&s.name)
            } else {
                false
            };
            SidebarEntry {
                name: s.name,
                running,
                local: s.local,
            }
        })
        .collect();
    // If no sessions found (shouldn't happen), add current as fallback
    if entries.is_empty() {
        entries.push(SidebarEntry {
            name: current_session.to_string(),
            running: true,
            local: true,
        });
    }
    let selected = entries
        .iter()
        .position(|e| e.name == current_session)
        .unwrap_or(0);
    (entries, selected)
}

/// Calculate sidebar width from entries (min 20, max 40).
fn sidebar_width(entries: &[SidebarEntry]) -> u16 {
    let max_name = entries.iter().map(|e| e.name.len()).max().unwrap_or(8);
    // " name " → 1 + name + 1 = name + 2
    let w = (max_name + 2).clamp(30, 50);
    w as u16
}

/// Draw the sidebar as a full-height left panel.
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
    let bg_style = Style::default().bg(Color::Black).fg(Color::White);

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

    // Session entries from the first row
    for (idx, entry) in sidebar.sessions.iter().enumerate() {
        let row_y = area.y + idx as u16;
        if row_y >= area.y + area.height {
            break;
        }

        let is_selected = idx == sidebar.selected;
        let line = format!(" {} ", entry.name);

        let style = if is_selected {
            Style::default().bg(Color::White).fg(Color::Black)
        } else {
            bg_style
        };

        // Fill entire row with background first
        for x in area.x..area.x + area.width {
            if x < buf.area().width && row_y < buf.area().height {
                let cell = &mut buf[(x, row_y)];
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
        // Write the text
        for (col, ch) in line.chars().enumerate() {
            let x = area.x + col as u16;
            if x >= area.x + area.width {
                break;
            }
            if x < buf.area().width && row_y < buf.area().height {
                let cell = &mut buf[(x, row_y)];
                cell.set_symbol(&ch.to_string());
                cell.set_style(style);
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
    Close,
    Switch(String),
    Redraw,
    None,
}

fn process_sidebar_input(
    data: &[u8],
    sidebar: &mut SidebarState,
    current_session: &str,
    sb_width: u16,
) -> SidebarAction {
    let mut i = 0;
    let mut result = SidebarAction::None;
    while i < data.len() {
        let b = data[i];

        // ESC or 'q' → close sidebar
        if b == 0x1b {
            // Check if it's a CSI sequence (arrow keys)
            if i + 2 < data.len() && data[i + 1] == b'[' {
                match data[i + 2] {
                    b'A' => {
                        // Up arrow
                        if sidebar.selected > 0 {
                            sidebar.selected -= 1;
                        }
                        result = SidebarAction::Redraw;
                        i += 3;
                        continue;
                    }
                    b'B' => {
                        // Down arrow
                        if sidebar.selected + 1 < sidebar.sessions.len() {
                            sidebar.selected += 1;
                        }
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
            // Bare ESC → close
            return SidebarAction::Close;
        }
        if b == b'q' {
            return SidebarAction::Close;
        }
        // j → down
        if b == b'j' {
            if sidebar.selected + 1 < sidebar.sessions.len() {
                sidebar.selected += 1;
            }
            result = SidebarAction::Redraw;
            i += 1;
            continue;
        }
        // k → up
        if b == b'k' {
            if sidebar.selected > 0 {
                sidebar.selected -= 1;
            }
            result = SidebarAction::Redraw;
            i += 1;
            continue;
        }
        // Enter → select
        if b == b'\r' || b == b'\n' {
            let selected_name = &sidebar.sessions[sidebar.selected].name;
            if selected_name == current_session {
                return SidebarAction::Close;
            }
            // Check if session can be started
            let entry = &sidebar.sessions[sidebar.selected];
            if !entry.running && !entry.local {
                // Non-local stopped session — can't switch
                i += 1;
                continue;
            }
            return SidebarAction::Switch(selected_name.clone());
        }
        i += 1;
    }
    result
}

/// Parse SGR mouse event within sidebar context.
/// Sidebar spans full height from row 1. Entries start at row 1 (1-indexed).
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
                // Sidebar: entries start at row 1 (1-indexed)
                if button == 0 && pressed && col <= sb_width && row >= 1 {
                    let entry_idx = (row - 1) as usize;
                    if entry_idx < sidebar.sessions.len() {
                        let selected_name = &sidebar.sessions[entry_idx].name;
                        if selected_name == current_session {
                            return Some((SidebarAction::Close, consumed));
                        }
                        let entry = &sidebar.sessions[entry_idx];
                        if !entry.running && !entry.local {
                            return Some((SidebarAction::None, consumed));
                        }
                        return Some((SidebarAction::Switch(selected_name.clone()), consumed));
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

pub fn run(session_name: &str, socket_path: &Path, tty_fd: i32) -> Result<ClientResult> {
    let (term_cols, term_rows) = terminal::get_term_size(tty_fd)?;

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

    let project_name = super::project_name_for_session(session_name);
    let header_color = super::color_for_session(session_name);
    let prefix_key = crate::config::load_mux_prefix_key();
    let mut input_state = InputState::new(prefix_key);

    // Draw the first frame immediately so the user sees content right
    // after a session switch instead of a blank screen.
    terminal::set_mouse_tracking(tty_fd, true);
    let mut mouse_tracking_on = true;
    {
        let max_scrollback = scrollback_line_count(&mut parser);
        parser.set_scrollback(0);
        let screen = parser.screen();
        let scroll = ScrollState {
            offset: 0,
            max: max_scrollback,
        };
        let params = DrawFrameParams {
            screen,
            session_name,
            project_name: &project_name,
            scroll: &scroll,
            command_mode: false,
            hover_close: false,
            header_color,
        };
        terminal::begin_sync_update(tty_fd);
        terminal
            .draw(|f| {
                terminal::draw_frame(f, &params, f.area());
            })
            .context("Failed to draw initial frame")?;
        terminal::end_sync_update(tty_fd);
        parser.set_scrollback(0);
    }

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

    let mut dirty = false;

    let mut last_cols = term_cols;
    let mut last_rows = term_rows;
    let mut current_inner_rows = inner_rows;

    // Sidebar state
    let mut sidebar: Option<SidebarState> = None;

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
                // When sidebar is open, intercept all input for sidebar navigation
                if let Some(ref mut sb) = sidebar {
                    let sb_width = sidebar_width(&sb.sessions);
                    match process_sidebar_input(&data, sb, session_name, sb_width) {
                        SidebarAction::Close => {
                            sidebar = None;
                            dirty = true;
                            // Force full redraw to clear sidebar overlay
                            terminal.clear()?;
                        }
                        SidebarAction::Switch(next) => {
                            // Close the dup'd input fd to unblock the reader thread
                            unsafe { libc::close(tty_input_fd) };
                            return Ok(ClientResult::SwitchSession(next));
                        }
                        SidebarAction::Redraw => {
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
                            return Ok(ClientResult::Exit(0));
                        }
                        InputAction::Kill => {
                            let _ = protocol::write_client_msg(&mut sock_writer, &ClientMsg::Kill);
                        }
                        InputAction::Redraw => {
                            dirty = true;
                        }
                        InputAction::OpenSidebar => {
                            let (entries, selected) = build_sidebar_entries(session_name);
                            sidebar = Some(SidebarState {
                                sessions: entries,
                                selected,
                            });
                            dirty = true;
                        }
                    }
                }
            }
            Ok(ClientEvent::ServerDisconnected) => {
                return Ok(ClientResult::Exit(0));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Flush any buffered incomplete escape sequence
                if sidebar.is_none() {
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
                        if new_inner > 0 && cols > 0 {
                            current_inner_rows = new_inner;
                            let _ = protocol::write_client_msg(
                                &mut sock_writer,
                                &ClientMsg::Resize {
                                    cols,
                                    rows: new_inner,
                                },
                            );
                            parser.set_size(new_inner, cols);
                            if cols_changed {
                                parser.process(b"\x1b[H\x1b[2J");
                            }
                            terminal = terminal::create_terminal(tty_fd, cols, rows)?;
                            terminal.clear()?;
                        }
                        input_state.scroll_offset = 0;
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
                    };
                    let sidebar_ref = sidebar.as_ref();
                    terminal::begin_sync_update(tty_fd);
                    terminal
                        .draw(|f| {
                            let full = f.area();
                            if let Some(sb) = sidebar_ref {
                                let sb_width = sidebar_width(&sb.sessions).min(full.width);
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
                                draw_sidebar(f, sb, sb_area, session_name);
                                terminal::draw_frame(f, &params, right_area);
                            } else {
                                terminal::draw_frame(f, &params, full);
                            }
                        })
                        .context("Failed to draw terminal frame")?;
                    terminal::end_sync_update(tty_fd);
                    parser.set_scrollback(0);
                    dirty = false;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(ClientResult::Exit(0))
}
