use anyhow::Result;
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::{TerminalOptions, Viewport};
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::parallel::{run_parallel, run_parallel_with_events, ProgressEvent, TaskResult};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: Duration = Duration::from_millis(80);
const BAR_WIDTH: usize = 20;
const MAX_VIEWPORT_LINES: u16 = 24;

#[derive(Clone, Copy, PartialEq)]
enum ItemState {
    Pending,
    Running,
    Done,
    Failed,
}

/// Run tasks in parallel with a live progress UI (when stderr is a TTY and
/// `verbose` is false). Falls back to the legacy single-line `label … ok`
/// output in verbose or non-TTY contexts. Returns results in input order.
///
/// When `show_items` is false, only the progress bar is rendered (no per-item
/// list beneath it) — useful for fast operations where the list is just noise.
pub fn run_parallel_with_progress<T, F>(
    label: &str,
    items: Vec<(String, T)>,
    verbose: bool,
    show_items: bool,
    task: F,
) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    let count = items.len();
    if count == 0 {
        return Vec::new();
    }

    let use_ui = !verbose && io::stderr().is_terminal();

    if !use_ui {
        if !verbose {
            eprint!("\x1b[2m{}…\x1b[0m ", label);
        }
        let results = run_parallel(items, task);
        if !verbose {
            let failures = results.iter().filter(|r| !r.success).count();
            if failures == 0 {
                eprintln!("\x1b[32mok\x1b[0m");
            } else {
                eprintln!("\x1b[31m{} failed\x1b[0m", failures);
            }
        }
        return results;
    }

    let names: Vec<String> = items.iter().map(|(n, _)| n.clone()).collect();
    let (tx, rx) = mpsc::channel::<ProgressEvent>();
    let label_owned = label.to_string();

    let renderer: JoinHandle<()> = thread::spawn(move || {
        if let Err(e) = render_loop(label_owned, names, show_items, rx) {
            eprintln!("progress renderer error: {}", e);
        }
    });

    let results = run_parallel_with_events(items, tx, task);
    let _ = renderer.join();

    let failures = results.iter().filter(|r| !r.success).count();
    if failures == 0 {
        eprintln!("\x1b[2m{}\x1b[0m \x1b[32mok\x1b[0m", label);
    } else {
        eprintln!("\x1b[2m{}\x1b[0m \x1b[31m{} failed\x1b[0m", label, failures);
    }
    results
}

fn render_loop(
    label: String,
    names: Vec<String>,
    show_items: bool,
    rx: Receiver<ProgressEvent>,
) -> Result<()> {
    let total = names.len();
    let term_height = terminal::size().map(|(_, h)| h).unwrap_or(24);
    let viewport_height = if show_items {
        let desired = (total as u16).saturating_add(1);
        desired
            .min(MAX_VIEWPORT_LINES)
            .min(term_height.saturating_sub(1).max(2))
    } else {
        1
    };

    terminal::enable_raw_mode()?;
    let _guard = RawGuard;

    let options = TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    };
    let mut terminal = Terminal::with_options(CrosstermBackend::new(io::stderr()), options)?;

    let mut states: Vec<(String, ItemState)> =
        names.into_iter().map(|n| (n, ItemState::Pending)).collect();

    let mut frame: usize = 0;
    let mut disconnected = false;

    loop {
        // Wait up to one tick for an event, then drain any additional events
        // that arrived during the same burst. The timeout drives spinner animation.
        match rx.recv_timeout(TICK) {
            Ok(ev) => {
                apply_event(&mut states, ev);
                while let Ok(ev) = rx.try_recv() {
                    apply_event(&mut states, ev);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => disconnected = true,
        }

        draw(&mut terminal, &label, &states, frame)?;
        frame = frame.wrapping_add(1);

        if disconnected {
            break;
        }
    }

    // Erase the inline viewport so the final summary line can take its place.
    terminal.clear()?;
    execute!(
        io::stderr(),
        cursor::MoveUp(viewport_height),
        terminal::Clear(terminal::ClearType::FromCursorDown),
    )?;
    Ok(())
}

fn apply_event(states: &mut [(String, ItemState)], ev: ProgressEvent) {
    match ev {
        ProgressEvent::Start(name) => {
            if let Some(e) = states.iter_mut().find(|(n, _)| n == &name) {
                e.1 = ItemState::Running;
            }
        }
        ProgressEvent::Finish(name, success) => {
            if let Some(e) = states.iter_mut().find(|(n, _)| n == &name) {
                e.1 = if success {
                    ItemState::Done
                } else {
                    ItemState::Failed
                };
            }
        }
    }
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    label: &str,
    states: &[(String, ItemState)],
    frame: usize,
) -> Result<()> {
    let total = states.len();
    let done = states
        .iter()
        .filter(|(_, s)| matches!(s, ItemState::Done | ItemState::Failed))
        .count();
    let pct = (done * 100).checked_div(total).unwrap_or(100);
    let filled = (done * BAR_WIDTH).checked_div(total).unwrap_or(BAR_WIDTH);
    let bar: String = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled);
    let spinner_frame = SPINNER[frame % SPINNER.len()];
    let active = done < total;

    terminal.draw(|f| {
        let area = f.area();
        if area.height == 0 {
            return;
        }

        let (head_icon, head_color) = if active {
            (spinner_frame, Color::Cyan)
        } else {
            ("✓", Color::Green)
        };
        let header = Line::from(vec![
            Span::styled(head_icon.to_string(), Style::default().fg(head_color)),
            Span::raw(" "),
            Span::styled(label.to_string(), Style::default().bold()),
            Span::raw(" "),
            Span::styled(format!("[{}]", bar), Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {}/{} ", done, total)),
            Span::styled(format!("({}%)", pct), Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(header, Rect::new(area.x, area.y, area.width, 1));

        let max_rows = area.height.saturating_sub(1) as usize;
        for (i, (name, state)) in states.iter().take(max_rows).enumerate() {
            let (icon, color) = match state {
                ItemState::Pending => ("·", Color::DarkGray),
                ItemState::Running => (spinner_frame, Color::Cyan),
                ItemState::Done => ("✓", Color::Green),
                ItemState::Failed => ("✗", Color::Red),
            };
            let name_style = match state {
                ItemState::Pending => Style::default().fg(Color::DarkGray),
                ItemState::Failed => Style::default().fg(Color::Red),
                _ => Style::default(),
            };
            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(icon.to_string(), Style::default().fg(color)),
                Span::raw(" "),
                Span::styled(name.clone(), name_style),
            ]);
            let y = area.y + (i as u16) + 1;
            if y < area.y + area.height {
                f.render_widget(line, Rect::new(area.x, y, area.width, 1));
            }
        }
    })?;
    Ok(())
}

struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
