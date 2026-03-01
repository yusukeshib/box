use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::{TerminalOptions, Viewport};
use std::io;
use std::path::PathBuf;

use crate::config;
use crate::session;

pub enum TuiAction {
    New {
        name: String,
        image: Option<String>,
        command: Option<Vec<String>>,
        local: bool,
        strategy: Option<String>,
    },
    Quit,
}

#[derive(PartialEq)]
enum Mode {
    Name,
    Image,
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

/// Minimal create-session TUI: prompts for name, (image), command.
/// Returns `TuiAction::New` or `TuiAction::Quit`.
pub fn create_session() -> Result<TuiAction> {
    let viewport_height = 1;

    terminal::enable_raw_mode()?;
    let _guard = TermGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut input = TextInput::new();
    let mut mode = Mode::Name;
    let mut footer_msg = String::new();
    let mut new_name = String::new();
    let mut new_image: Option<String> = None;
    let mut new_local = false;

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let line: Line = if !footer_msg.is_empty() {
                Line::from(Span::styled(
                    footer_msg.as_str(),
                    Style::default().fg(Color::Red),
                ))
            } else {
                match &mode {
                    Mode::Name => Line::from(input.to_spans("Session name: ")),
                    Mode::Image => Line::from(input.to_spans("Image: ")),
                    Mode::Command => Line::from(input.to_spans("Command (optional): ")),
                }
            };
            f.render_widget(line, area);
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
                Mode::Name => match key.code {
                    KeyCode::Enter => {
                        let raw_name = input.text.trim().to_string();
                        let name = session::normalize_name(&raw_name);
                        if let Err(e) = session::validate_name(&name) {
                            footer_msg = e.to_string();
                            input = TextInput::new();
                        } else if session::session_exists(&name).unwrap_or(false) {
                            footer_msg = format!("Session '{}' already exists.", name);
                            input = TextInput::new();
                        } else if std::env::var("BOX_MODE")
                            .map(|v| v != "docker")
                            .unwrap_or(true)
                        {
                            new_name = name;
                            new_image = None;
                            new_local = true;
                            let default_cmd = std::env::var("BOX_DEFAULT_CMD").unwrap_or_default();
                            input = TextInput::with_text(default_cmd);
                            mode = Mode::Command;
                        } else {
                            new_name = name;
                            let default_image = std::env::var("BOX_DEFAULT_IMAGE")
                                .unwrap_or_else(|_| config::DEFAULT_IMAGE.to_string());
                            input = TextInput::with_text(default_image);
                            mode = Mode::Image;
                        }
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Quit);
                    }
                    KeyCode::Up => {
                        if let Some(entry) = name_history.up(&input.text) {
                            input = TextInput::with_text(entry.to_string());
                        }
                    }
                    KeyCode::Down => {
                        if let Some(entry) = name_history.down(&input.text) {
                            input = TextInput::with_text(entry.to_string());
                        }
                    }
                    _ => {
                        input.handle_key(key.code);
                    }
                },
                Mode::Image => match key.code {
                    KeyCode::Enter => {
                        let image_text = input.text.trim().to_string();
                        new_image = if image_text.is_empty() {
                            None
                        } else {
                            Some(image_text)
                        };
                        let default_cmd = std::env::var("BOX_DEFAULT_CMD").unwrap_or_default();
                        input = TextInput::with_text(default_cmd);
                        mode = Mode::Command;
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Quit);
                    }
                    _ => {
                        input.handle_key(key.code);
                    }
                },
                Mode::Command => match key.code {
                    KeyCode::Enter => {
                        let cmd_text = input.text.trim().to_string();
                        new_command_text = cmd_text.clone();
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
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::New {
                            name: new_name,
                            image: new_image,
                            command,
                            local: new_local,
                            strategy: None,
                        });
                    }
                    KeyCode::Esc => {
                        clear_viewport(&mut terminal, viewport_height)?;
                        return Ok(TuiAction::Quit);
                    }
                    KeyCode::Up => {
                        if let Some(entry) = command_history.up(&input.text) {
                            input = TextInput::with_text(entry.to_string());
                        }
                    }
                    KeyCode::Down => {
                        if let Some(entry) = command_history.down(&input.text) {
                            input = TextInput::with_text(entry.to_string());
                        }
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

    fn make_history(entries: Vec<&str>) -> InputHistory {
        let entries: Vec<String> = entries.into_iter().map(String::from).collect();
        let position = entries.len();
        InputHistory {
            entries,
            position,
            draft: String::new(),
        }
    }

    #[test]
    fn test_history_up_down() {
        let mut h = make_history(vec!["alpha", "beta", "gamma"]);
        assert_eq!(h.up(""), Some("gamma"));
        assert_eq!(h.up(""), Some("beta"));
        assert_eq!(h.up(""), Some("alpha"));
        // Already at oldest, stays there
        assert_eq!(h.up(""), Some("alpha"));
        // Navigate back down
        assert_eq!(h.down(""), Some("beta"));
        assert_eq!(h.down(""), Some("gamma"));
        // Past newest returns draft
        assert_eq!(h.down(""), Some(""));
        // Past draft returns None
        assert_eq!(h.down(""), None);
    }

    #[test]
    fn test_history_draft_preservation() {
        let mut h = make_history(vec!["old"]);
        // User is typing "new" then presses Up
        assert_eq!(h.up("new"), Some("old"));
        // Press Down to return to draft
        assert_eq!(h.down("old"), Some("new"));
    }

    #[test]
    fn test_history_empty() {
        let mut h = make_history(vec![]);
        assert_eq!(h.up("text"), None);
        assert_eq!(h.down("text"), None);
    }

    #[test]
    fn test_history_push_dedup() {
        let mut h = make_history(vec!["alpha", "beta"]);
        h.push("alpha"); // duplicate
        assert_eq!(h.entries, vec!["beta", "alpha"]);
    }

    #[test]
    fn test_history_push_empty_ignored() {
        let mut h = make_history(vec!["alpha"]);
        h.push("");
        h.push("   ");
        assert_eq!(h.entries, vec!["alpha"]);
    }

    #[test]
    fn test_history_push_cap() {
        let mut h = make_history(vec![]);
        for i in 0..110 {
            h.push(&format!("entry-{}", i));
        }
        assert_eq!(h.entries.len(), HISTORY_MAX);
        assert_eq!(h.entries[0], "entry-10");
        assert_eq!(h.entries[HISTORY_MAX - 1], "entry-109");
    }

    #[test]
    fn test_history_reset_position() {
        let mut h = make_history(vec!["alpha", "beta"]);
        h.up("x");
        h.up("x");
        assert_eq!(h.position, 0);
        h.reset_position();
        assert_eq!(h.position, 2);
        assert!(h.draft.is_empty());
    }
}
