use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::{TerminalOptions, Viewport};
use std::io;

use crate::config;
use crate::session;

pub enum TuiAction {
    New {
        name: String,
        image: Option<String>,
        command: Option<Vec<String>>,
        local: bool,
        strategy: Option<crate::config::Strategy>,
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
                        let name = input.text.trim().to_string();
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
