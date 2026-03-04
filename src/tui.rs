use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::{TerminalOptions, Viewport};
use std::io;

use crate::repo;
use crate::session;

pub enum TuiAction {
    New {
        name: String,
        command: Option<Vec<String>>,
        repos: Vec<String>,
    },
    Quit,
}

#[derive(PartialEq)]
enum Mode {
    RepoSelect,
    Name,
    Command,
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

/// Minimal create-session TUI: prompts for repo selection, name, command.
/// Returns `TuiAction::New` or `TuiAction::Quit`.
pub fn create_session() -> Result<TuiAction> {
    let all_repos = repo::list()?;
    if all_repos.is_empty() {
        anyhow::bail!("No repos registered. Run `box repo add <path>` first.");
    }

    let repo_count = all_repos.len();
    // viewport height: repo list + header line
    let viewport_height = (repo_count as u16) + 1;

    terminal::enable_raw_mode()?;
    let _guard = TermGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut input = TextInput::new();
    let mut mode = Mode::RepoSelect;
    let mut footer_msg = String::new();
    let mut new_name = String::new();

    // Repo selection state
    let mut selected: Vec<bool> = vec![true; repo_count];
    let mut cursor_pos: usize = 0;
    let mut selected_repos: Vec<String> = Vec::new();

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
                Mode::Command => {
                    let line = Line::from(input.to_spans("Command (optional): "));
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
                        let name = input.text.trim().to_string();
                        if let Err(e) = session::validate_name(&name) {
                            footer_msg = e.to_string();
                            input = TextInput::new();
                        } else if session::session_exists(&name).unwrap_or(false) {
                            footer_msg = format!("Session '{}' already exists.", name);
                            input = TextInput::new();
                        } else {
                            new_name = name;
                            let default_cmd = std::env::var("BOX_DEFAULT_CMD").unwrap_or_default();
                            input = TextInput::with_text(default_cmd);
                            mode = Mode::Command;
                        }
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, 1)?;
                        return Ok(TuiAction::Quit);
                    }
                    _ => {
                        input.handle_key(key.code);
                    }
                },
                Mode::Command => match key.code {
                    KeyCode::Enter => {
                        let cmd_text = input.text.trim().to_string();
                        let command = if cmd_text.is_empty() {
                            Some(vec![])
                        } else {
                            match shell_words::split(&cmd_text) {
                                Ok(args) => Some(args),
                                Err(e) => {
                                    footer_msg = format!("Invalid command: {e}");
                                    input = TextInput::new();
                                    continue;
                                }
                            }
                        };
                        clear_viewport(&mut terminal, 1)?;
                        return Ok(TuiAction::New {
                            name: new_name,
                            command,
                            repos: selected_repos,
                        });
                    }
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
