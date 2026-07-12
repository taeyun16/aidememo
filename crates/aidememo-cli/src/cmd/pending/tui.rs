//! Terminal rendering + input loop for `aidememo pending review`.
//!
//! Three panes stacked top-to-bottom:
//!   1. List — every pending entry, with its current selection mark
//!      (commit / discard / unmarked) and a one-line summary.
//!   2. Detail — the source line that triggered the focused entry,
//!      its type, confidence, and a relative timestamp.
//!   3. Help — keybinding cheatsheet.
//!
//! Keybindings — kept as close to mutt / lazygit norms as possible:
//!
//!   ↑/k       move cursor up
//!   ↓/j       move cursor down
//!   space     cycle current entry (— → commit → discard → —)
//!   a         mark every entry for commit
//!   A         clear every selection
//!   c / Enter apply (commits + discards) and exit
//!   q / Esc   quit without applying
//!   ?         toggle the help pane

use super::AppState;
use aidememo_core::AideMemoError;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::io;
use std::time::Duration;

pub fn run(initial: AppState) -> Result<AppState, AideMemoError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| AideMemoError::Internal(format!("enable_raw_mode: {e}")))?;
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| AideMemoError::Internal(format!("enter alt screen: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| AideMemoError::Internal(format!("new terminal: {e}")))?;

    let result = run_loop(&mut terminal, initial);

    // Always restore the terminal — whatever happened above, we
    // can't leave the user in raw mode + alt screen.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut state: AppState,
) -> Result<AppState, AideMemoError> {
    loop {
        terminal
            .draw(|frame| render(frame, &state))
            .map_err(|e| AideMemoError::Internal(format!("draw: {e}")))?;

        // 100 ms poll keeps the loop responsive enough for keypresses
        // without burning CPU when idle.
        if event::poll(Duration::from_millis(100))
            .map_err(|e| AideMemoError::Internal(format!("poll: {e}")))?
            && let Event::Key(key) =
                event::read().map_err(|e| AideMemoError::Internal(format!("read: {e}")))?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            handle_key(&mut state, key.code, key.modifiers);
        }

        if state.quit {
            return Ok(state);
        }
    }
}

pub(super) fn handle_key(state: &mut AppState, code: KeyCode, modifiers: KeyModifiers) {
    match code {
        KeyCode::Up | KeyCode::Char('k') => state.cursor_up(),
        KeyCode::Down | KeyCode::Char('j') => state.cursor_down(),
        KeyCode::Char(' ') => {
            state.cycle_current();
            state.status = format!(
                "#{}: {}",
                state.cursor + 1,
                state.pending_action(state.cursor)
            );
        }
        KeyCode::Char('a') if !modifiers.contains(KeyModifiers::SHIFT) => {
            state.select_all_commit();
            state.status = format!("marked {} for commit", state.entries.len());
        }
        KeyCode::Char('A') => {
            state.clear_selections();
            state.status = "cleared all selections".to_string();
        }
        KeyCode::Char('c') | KeyCode::Enter => {
            state.apply_on_quit = true;
            state.quit = true;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            state.apply_on_quit = false;
            state.quit = true;
        }
        _ => {}
    }
}

fn render(frame: &mut ratatui::Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // list
            Constraint::Length(6), // detail
            Constraint::Length(3), // help
        ])
        .split(frame.area());

    render_list(frame, chunks[0], state);
    render_detail(frame, chunks[1], state);
    render_help(frame, chunks[2], state);
}

fn render_list(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .entries
        .iter()
        .enumerate()
        .map(|(idx, e)| {
            let mark = if state.commit.contains(&idx) {
                Span::styled("[+]", Style::default().fg(Color::Green))
            } else if state.discard.contains(&idx) {
                Span::styled("[-]", Style::default().fg(Color::Red))
            } else {
                Span::styled("[ ]", Style::default().fg(Color::DarkGray))
            };
            let kind = Span::styled(
                format!("{:<10}", e.fact_type),
                Style::default().fg(Color::Cyan),
            );
            let conf = Span::styled(
                format!("{:.2}", e.confidence),
                Style::default().fg(Color::Yellow),
            );
            let content = truncate(&e.content, area.width.saturating_sub(28) as usize);
            ListItem::new(Line::from(vec![
                mark,
                Span::raw(" "),
                Span::raw(format!("#{:<3}", idx + 1)),
                Span::raw(" "),
                kind,
                Span::raw(" "),
                conf,
                Span::raw("  "),
                Span::raw(content),
            ]))
        })
        .collect();

    let title = format!(
        " aidememo pending — {} entry(ies)  •  {} ",
        state.entries.len(),
        state.log_path.display()
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .fg(Color::White),
        )
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    list_state.select(if state.entries.is_empty() {
        None
    } else {
        Some(state.cursor)
    });
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_detail(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let lines: Vec<Line> = if let Some(entry) = state.entries.get(state.cursor) {
        vec![
            Line::from(vec![
                Span::styled("Source: ", Style::default().fg(Color::DarkGray)),
                Span::raw(entry.source_line.clone()),
            ]),
            Line::from(vec![
                Span::styled("Type: ", Style::default().fg(Color::DarkGray)),
                Span::styled(entry.fact_type.clone(), Style::default().fg(Color::Cyan)),
                Span::raw("    "),
                Span::styled("Confidence: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.2}", entry.confidence),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("    "),
                Span::styled("Captured: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_relative_time(entry.ts_ms)),
            ]),
            Line::from(vec![
                Span::styled("Action: ", Style::default().fg(Color::DarkGray)),
                Span::raw(state.pending_action(state.cursor).to_string()),
            ]),
        ]
    } else {
        vec![Line::from("(no entries)")]
    };
    let detail = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" detail "))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn render_help(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let mut spans = vec![
        Span::styled("↑/k ↓/j", Style::default().fg(Color::Yellow)),
        Span::raw(" move  "),
        Span::styled("space", Style::default().fg(Color::Yellow)),
        Span::raw(" cycle  "),
        Span::styled("a", Style::default().fg(Color::Yellow)),
        Span::raw(" all  "),
        Span::styled("A", Style::default().fg(Color::Yellow)),
        Span::raw(" none  "),
        Span::styled("c/⏎", Style::default().fg(Color::Green)),
        Span::raw(" apply  "),
        Span::styled("q/Esc", Style::default().fg(Color::Red)),
        Span::raw(" cancel"),
    ];
    if !state.status.is_empty() {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(
            state.status.clone(),
            Style::default().add_modifier(Modifier::ITALIC),
        ));
    }
    let help = Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL));
    frame.render_widget(help, area);
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut)
}

fn format_relative_time(ts_ms: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ts_ms);
    let delta = now_ms.saturating_sub(ts_ms);
    let secs = delta / 1000;
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::pending::PendingEntry;
    use std::path::PathBuf;

    fn entry(content: &str, kind: &str, conf: f32) -> PendingEntry {
        PendingEntry {
            ts_ms: 1_700_000_000_000,
            content: content.to_string(),
            fact_type: kind.to_string(),
            confidence: conf,
            source_line: format!("src: {}", content),
        }
    }

    #[test]
    fn enter_sets_apply_and_quits() {
        let mut s = AppState::new(vec![entry("a", "note", 0.9)], PathBuf::from("/x"));
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(s.commit.contains(&0));
        handle_key(&mut s, KeyCode::Enter, KeyModifiers::NONE);
        assert!(s.quit);
        assert!(s.apply_on_quit);
    }

    #[test]
    fn esc_quits_without_apply() {
        let mut s = AppState::new(vec![entry("a", "note", 0.9)], PathBuf::from("/x"));
        s.commit.insert(0);
        handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE);
        assert!(s.quit);
        assert!(!s.apply_on_quit);
    }

    #[test]
    fn lowercase_a_selects_all_uppercase_a_clears() {
        let mut s = AppState::new(
            vec![entry("a", "note", 0.5), entry("b", "note", 0.5)],
            PathBuf::from("/x"),
        );
        handle_key(&mut s, KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(s.commit.len(), 2);
        handle_key(&mut s, KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert!(s.commit.is_empty());
    }

    #[test]
    fn truncate_handles_unicode_safely() {
        // 영어 wiki / multilingual: scalar truncation must not slice
        // mid-character. (Single-byte truncation would panic on the
        // boundary; chars()-based truncation should not.)
        let s = "결정: 영어 wiki에서도 multilingual-128M로 가자";
        let out = truncate(s, 8);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 8);
    }
}
