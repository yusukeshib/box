use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::{TerminalOptions, Viewport};
use std::io;

use crate::config;
use crate::preset;
use crate::repo;
use crate::session;

const MAX_HISTORY: usize = 100;

fn last_selected_repos_path() -> Result<std::path::PathBuf> {
    Ok(config::box_root()?.join("last_selected_repos"))
}

fn name_history_path() -> Result<std::path::PathBuf> {
    Ok(config::box_root()?.join("name_history"))
}

fn load_last_selected_repos() -> Vec<String> {
    last_selected_repos_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_last_selected_repos(repos: &[String]) {
    if let Ok(path) = last_selected_repos_path() {
        let _ = std::fs::write(path, repos.join("\n") + "\n");
    }
}

fn load_name_history() -> Vec<String> {
    name_history_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_name_history(history: &[String]) {
    if let Ok(path) = name_history_path() {
        let capped: Vec<&String> = history.iter().take(MAX_HISTORY).collect();
        let content: Vec<&str> = capped.iter().map(|s| s.as_str()).collect();
        let _ = std::fs::write(path, content.join("\n") + "\n");
    }
}

pub enum TuiAction {
    New { name: String, repos: Vec<String> },
    Edit { repos: Vec<String> },
    Remove { sessions: Vec<String> },
    Quit,
}

#[derive(PartialEq)]
enum Mode {
    PresetSelect,
    RepoSelect,
    Name,
}

struct TextInput {
    text: String,
    cursor: usize,
}

impl TextInput {
    fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    fn with_text(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    fn handle_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.text.drain(prev..self.cursor);
                    self.cursor = prev;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    let next = self.text[self.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor + i)
                        .unwrap_or(self.text.len());
                    self.text.drain(self.cursor..next);
                }
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor = self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.cursor < self.text.len() {
                    self.cursor = self.text[self.cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.cursor + i)
                        .unwrap_or(self.text.len());
                }
            }
            _ => {}
        }
    }

    fn to_spans(&self, prefix: &str) -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled(prefix.to_string(), Style::default().bold())];
        let text = &self.text;
        if self.cursor < text.len() {
            let next = text[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(text.len());
            spans.push(Span::raw(text[..self.cursor].to_string()));
            spans.push(Span::styled(
                text[self.cursor..next].to_string(),
                Style::default().reversed(),
            ));
            spans.push(Span::raw(text[next..].to_string()));
        } else {
            spans.push(Span::raw(text.clone()));
            spans.push(Span::styled(" ", Style::default().reversed()));
        }
        spans
    }
}

struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

fn clear_viewport(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    height: u16,
) -> Result<()> {
    terminal.clear()?;
    execute!(
        io::stderr(),
        cursor::MoveUp(height),
        terminal::Clear(terminal::ClearType::FromCursorDown)
    )?;
    Ok(())
}

/// Minimal create-session TUI: prompts for preset/repo selection, name.
/// Returns `TuiAction::New` or `TuiAction::Quit`.
pub fn create_session() -> Result<TuiAction> {
    let all_repos = repo::list()?;
    if all_repos.is_empty() {
        anyhow::bail!("No repos registered. Run `box repo add <path>` first.");
    }

    let presets = preset::list().unwrap_or_default();
    let repo_count = all_repos.len();

    // Start in PresetSelect if presets exist, otherwise RepoSelect
    let has_presets = !presets.is_empty();
    // preset_items = number of presets + 1 for "Custom..."
    let preset_item_count = presets.len() + 1;
    let initial_height = if has_presets {
        (preset_item_count as u16) + 1 // items + header
    } else {
        (repo_count as u16) + 1 // repos + header
    };
    let mut viewport_height = initial_height;

    terminal::enable_raw_mode()?;
    let _guard = TermGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut input = TextInput::new();
    let mut mode = if has_presets {
        Mode::PresetSelect
    } else {
        Mode::RepoSelect
    };
    let mut footer_msg = String::new();

    // Repo selection state — restore from last session if available
    let last_selected = load_last_selected_repos();
    let mut selected: Vec<bool> = if last_selected.is_empty() {
        vec![true; repo_count]
    } else {
        all_repos
            .iter()
            .map(|r| last_selected.contains(&r.name))
            .collect()
    };
    let mut cursor_pos: usize = 0;
    let mut selected_repos: Vec<String> = Vec::new();

    // Name history state
    let mut name_history = load_name_history();
    let mut name_history_index: Option<usize> = None;
    let mut name_saved_input = String::new();

    loop {
        terminal.draw(|f| {
            let area = f.area();

            if !footer_msg.is_empty() {
                let line = Line::from(Span::styled(
                    footer_msg.as_str(),
                    Style::default().fg(Color::Red),
                ));
                f.render_widget(line, area);
                return;
            }

            match &mode {
                Mode::PresetSelect => {
                    let mut lines: Vec<Line> = Vec::new();
                    lines.push(Line::from(Span::styled(
                        "Select a preset (Enter=confirm):",
                        Style::default().bold(),
                    )));
                    for (i, (name, repos)) in presets.iter().enumerate() {
                        let label = format!(" {} ({})", name, repos.join(", "));
                        let style = if i == cursor_pos {
                            Style::default().reversed()
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(Span::styled(label, style)));
                    }
                    // "Custom..." entry
                    let custom_style = if cursor_pos == presets.len() {
                        Style::default().reversed()
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(" Custom...", custom_style)));
                    for (i, line) in lines.into_iter().enumerate() {
                        if (i as u16) < area.height {
                            let row = Rect::new(area.x, area.y + i as u16, area.width, 1);
                            f.render_widget(line, row);
                        }
                    }
                }
                Mode::RepoSelect => {
                    let mut lines: Vec<Line> = Vec::new();
                    lines.push(Line::from(Span::styled(
                        "Select repos (Space=toggle, Enter=confirm):",
                        Style::default().bold(),
                    )));
                    for (i, repo) in all_repos.iter().enumerate() {
                        let check = if selected[i] { "[x]" } else { "[ ]" };
                        let style = if i == cursor_pos {
                            Style::default().reversed()
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(Span::styled(
                            format!(" {} {}", check, repo.name),
                            style,
                        )));
                    }
                    for (i, line) in lines.into_iter().enumerate() {
                        if (i as u16) < area.height {
                            let row = Rect::new(area.x, area.y + i as u16, area.width, 1);
                            f.render_widget(line, row);
                        }
                    }
                }
                Mode::Name => {
                    let line = Line::from(input.to_spans("Session name: "));
                    f.render_widget(line, area);
                }
            }
        })?;

        // Clear error message on next keypress
        if !footer_msg.is_empty() {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    footer_msg.clear();
                }
            }
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                clear_viewport(&mut terminal, viewport_height)?;
                return Ok(TuiAction::Quit);
            }

            match mode {
                Mode::PresetSelect => match key.code {
                    KeyCode::Up => {
                        cursor_pos = cursor_pos.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if cursor_pos + 1 < preset_item_count {
                            cursor_pos += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if cursor_pos < presets.len() {
                            // Selected a preset
                            let (_, repos) = &presets[cursor_pos];
                            let all_repo_names: Vec<&str> =
                                all_repos.iter().map(|r| r.name.as_str()).collect();
                            selected_repos = repos
                                .iter()
                                .filter(|r| all_repo_names.contains(&r.as_str()))
                                .cloned()
                                .collect();
                            if selected_repos.is_empty() {
                                footer_msg =
                                    "All repos in this preset have been removed.".to_string();
                            } else {
                                save_last_selected_repos(&selected_repos);
                                // Transition to Name mode
                                clear_viewport(&mut terminal, viewport_height)?;
                                drop(terminal);
                                terminal = Terminal::with_options(
                                    CrosstermBackend::new(io::stderr()),
                                    TerminalOptions {
                                        viewport: Viewport::Inline(1),
                                    },
                                )?;
                                viewport_height = 1;
                                mode = Mode::Name;
                            }
                        } else {
                            // "Custom..." selected — fall through to RepoSelect
                            clear_viewport(&mut terminal, viewport_height)?;
                            drop(terminal);
                            let new_height = (repo_count as u16) + 1;
                            terminal = Terminal::with_options(
                                CrosstermBackend::new(io::stderr()),
                                TerminalOptions {
                                    viewport: Viewport::Inline(new_height),
                                },
                            )?;
                            viewport_height = new_height;
                            cursor_pos = 0;
                            mode = Mode::RepoSelect;
                        }
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Quit);
                    }
                    _ => {}
                },
                Mode::RepoSelect => match key.code {
                    KeyCode::Up => {
                        cursor_pos = cursor_pos.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if cursor_pos + 1 < repo_count {
                            cursor_pos += 1;
                        }
                    }
                    KeyCode::Char(' ') => {
                        selected[cursor_pos] = !selected[cursor_pos];
                    }
                    KeyCode::Enter => {
                        selected_repos = all_repos
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| selected[*i])
                            .map(|(_, r)| r.name.clone())
                            .collect();
                        if selected_repos.is_empty() {
                            footer_msg = "At least one repo must be selected.".to_string();
                        } else {
                            save_last_selected_repos(&selected_repos);
                            // Resize viewport to 1 line for text input
                            clear_viewport(&mut terminal, viewport_height)?;
                            drop(terminal);
                            let options = TerminalOptions {
                                viewport: Viewport::Inline(1),
                            };
                            terminal = Terminal::with_options(
                                CrosstermBackend::new(io::stderr()),
                                options,
                            )?;
                            viewport_height = 1;
                            mode = Mode::Name;
                        }
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Quit);
                    }
                    _ => {}
                },
                Mode::Name => match key.code {
                    KeyCode::Enter => {
                        let raw_name = input.text.trim().to_string();
                        let name = match session::validate_name(&raw_name) {
                            Ok(n) => n,
                            Err(e) => {
                                footer_msg = e.to_string();
                                input = TextInput::new();
                                name_history_index = None;
                                name_saved_input.clear();
                                continue;
                            }
                        };
                        if session::session_exists(&name).unwrap_or(false) {
                            footer_msg = format!("Session '{}' already exists.", name);
                            input = TextInput::new();
                            name_history_index = None;
                            name_saved_input.clear();
                        } else {
                            // Save name to history
                            name_history.retain(|h| h != &name);
                            name_history.insert(0, name.clone());
                            save_name_history(&name_history);
                            clear_viewport(&mut terminal, 1)?;
                            return Ok(TuiAction::New {
                                name,
                                repos: selected_repos,
                            });
                        }
                    }
                    KeyCode::Up => {
                        if !name_history.is_empty() {
                            match name_history_index {
                                None => {
                                    name_saved_input = input.text.clone();
                                    name_history_index = Some(0);
                                    input = TextInput::with_text(name_history[0].clone());
                                }
                                Some(idx) if idx + 1 < name_history.len() => {
                                    name_history_index = Some(idx + 1);
                                    input = TextInput::with_text(name_history[idx + 1].clone());
                                }
                                _ => {}
                            }
                        }
                    }
                    KeyCode::Down => match name_history_index {
                        Some(0) => {
                            name_history_index = None;
                            input = TextInput::with_text(name_saved_input.clone());
                        }
                        Some(idx) => {
                            name_history_index = Some(idx - 1);
                            input = TextInput::with_text(name_history[idx - 1].clone());
                        }
                        None => {}
                    },
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, 1)?;
                        return Ok(TuiAction::Quit);
                    }
                    _ => {
                        input.handle_key(key.code);
                    }
                },
            }
        }
    }
}

/// TUI for editing session repos: shows checkbox list of all registered repos
/// with the session's current repos pre-selected. Returns updated repo list.
pub fn edit_session(current_repos: &[String]) -> Result<TuiAction> {
    let all_repos = repo::list()?;
    if all_repos.is_empty() {
        anyhow::bail!("No repos registered. Run `box repo add <path>` first.");
    }

    let repo_count = all_repos.len();
    let viewport_height = (repo_count as u16) + 1;

    terminal::enable_raw_mode()?;
    let _guard = TermGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut footer_msg = String::new();
    let mut selected: Vec<bool> = all_repos
        .iter()
        .map(|r| current_repos.contains(&r.name))
        .collect();
    let mut cursor_pos: usize = 0;

    loop {
        terminal.draw(|f| {
            let area = f.area();

            if !footer_msg.is_empty() {
                let line = Line::from(Span::styled(
                    footer_msg.as_str(),
                    Style::default().fg(Color::Red),
                ));
                f.render_widget(line, area);
                return;
            }

            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                "Edit repos (Space=toggle, Enter=confirm):",
                Style::default().bold(),
            )));
            for (i, repo) in all_repos.iter().enumerate() {
                let check = if selected[i] { "[x]" } else { "[ ]" };
                let style = if i == cursor_pos {
                    Style::default().reversed()
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!(" {} {}", check, repo.name),
                    style,
                )));
            }
            for (i, line) in lines.into_iter().enumerate() {
                if (i as u16) < area.height {
                    let row = Rect::new(area.x, area.y + i as u16, area.width, 1);
                    f.render_widget(line, row);
                }
            }
        })?;

        if !footer_msg.is_empty() {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    footer_msg.clear();
                }
            }
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                clear_viewport(&mut terminal, viewport_height)?;
                return Ok(TuiAction::Quit);
            }

            match key.code {
                KeyCode::Up => {
                    cursor_pos = cursor_pos.saturating_sub(1);
                }
                KeyCode::Down => {
                    if cursor_pos + 1 < repo_count {
                        cursor_pos += 1;
                    }
                }
                KeyCode::Char(' ') => {
                    selected[cursor_pos] = !selected[cursor_pos];
                }
                KeyCode::Enter => {
                    let selected_repos: Vec<String> = all_repos
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| selected[*i])
                        .map(|(_, r)| r.name.clone())
                        .collect();
                    if selected_repos.is_empty() {
                        footer_msg = "At least one repo must be selected.".to_string();
                    } else {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Edit {
                            repos: selected_repos,
                        });
                    }
                }
                KeyCode::Esc => {
                    clear_viewport(&mut terminal, viewport_height)?;
                    return Ok(TuiAction::Quit);
                }
                _ => {}
            }
        }
    }
}

/// TUI for selecting sessions to remove: shows checkbox list of all sessions.
/// Returns `TuiAction::Remove { sessions }` or `TuiAction::Quit`.
pub fn select_sessions() -> Result<TuiAction> {
    let all_sessions = session::list()?;
    if all_sessions.is_empty() {
        anyhow::bail!("No sessions found.");
    }

    let count = all_sessions.len();
    let viewport_height = (count as u16) + 1;

    terminal::enable_raw_mode()?;
    let _guard = TermGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut selected: Vec<bool> = vec![false; count];
    let mut cursor_pos: usize = 0;
    let mut footer_msg = String::new();

    loop {
        terminal.draw(|f| {
            let area = f.area();

            if !footer_msg.is_empty() {
                let line = Line::from(Span::styled(
                    footer_msg.as_str(),
                    Style::default().fg(Color::Red),
                ));
                f.render_widget(line, area);
                return;
            }

            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                "Select sessions to remove (Space=toggle, Enter=confirm):",
                Style::default().bold(),
            )));
            for (i, sess) in all_sessions.iter().enumerate() {
                let check = if selected[i] { "[x]" } else { "[ ]" };
                let label = if sess.repos.is_empty() {
                    sess.name.clone()
                } else {
                    format!("{} ({})", sess.name, sess.repos.join(", "))
                };
                let style = if i == cursor_pos {
                    Style::default().reversed()
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!(" {} {}", check, label),
                    style,
                )));
            }
            for (i, line) in lines.into_iter().enumerate() {
                if (i as u16) < area.height {
                    let row = Rect::new(area.x, area.y + i as u16, area.width, 1);
                    f.render_widget(line, row);
                }
            }
        })?;

        if !footer_msg.is_empty() {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    footer_msg.clear();
                }
            }
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                clear_viewport(&mut terminal, viewport_height)?;
                return Ok(TuiAction::Quit);
            }

            match key.code {
                KeyCode::Up => {
                    cursor_pos = cursor_pos.saturating_sub(1);
                }
                KeyCode::Down => {
                    if cursor_pos + 1 < count {
                        cursor_pos += 1;
                    }
                }
                KeyCode::Char(' ') => {
                    selected[cursor_pos] = !selected[cursor_pos];
                }
                KeyCode::Enter => {
                    let chosen: Vec<String> = all_sessions
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| selected[*i])
                        .map(|(_, s)| s.name.clone())
                        .collect();
                    if chosen.is_empty() {
                        footer_msg = "At least one session must be selected.".to_string();
                    } else {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Remove { sessions: chosen });
                    }
                }
                KeyCode::Esc => {
                    clear_viewport(&mut terminal, viewport_height)?;
                    return Ok(TuiAction::Quit);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_input_insert() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Char('b'));
        input.handle_key(KeyCode::Char('c'));
        assert_eq!(input.text, "abc");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn test_text_input_backspace() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Char('b'));
        input.handle_key(KeyCode::Backspace);
        assert_eq!(input.text, "a");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn test_text_input_backspace_at_start() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Backspace);
        assert_eq!(input.text, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn test_text_input_delete() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Char('b'));
        input.handle_key(KeyCode::Left);
        input.handle_key(KeyCode::Delete);
        assert_eq!(input.text, "a");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn test_text_input_delete_at_end() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Delete);
        assert_eq!(input.text, "a");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn test_text_input_cursor_movement() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Char('b'));
        input.handle_key(KeyCode::Char('c'));
        input.handle_key(KeyCode::Left);
        input.handle_key(KeyCode::Left);
        assert_eq!(input.cursor, 1);
        input.handle_key(KeyCode::Right);
        assert_eq!(input.cursor, 2);
    }

    #[test]
    fn test_text_input_left_at_start() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Left);
        input.handle_key(KeyCode::Left); // should not go below 0
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn test_text_input_right_at_end() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Right); // should not go past len
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn test_text_input_insert_at_cursor() {
        let mut input = TextInput::new();
        input.handle_key(KeyCode::Char('a'));
        input.handle_key(KeyCode::Char('c'));
        input.handle_key(KeyCode::Left);
        input.handle_key(KeyCode::Char('b'));
        assert_eq!(input.text, "abc");
        assert_eq!(input.cursor, 2);
    }
}
