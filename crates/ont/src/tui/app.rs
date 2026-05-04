use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::event::Message;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const HINT: &str = "press q or Ctrl-C to quit";

#[derive(Debug, Clone, Copy)]
pub enum Flow {
    Continue,
    Exit,
}

#[derive(Debug)]
pub struct App {
    pub last_message: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            last_message: "ready".to_string(),
        }
    }

    pub fn update(&mut self, message: Message) -> Flow {
        match message {
            Message::Key(key) => self.on_key(key),
            Message::Interrupt => Flow::Exit,
            Message::Tick | Message::Resize(_, _) | Message::Mouse(_) => Flow::Continue,
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Flow {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Flow::Exit,
            KeyCode::Char('c') if ctrl => Flow::Exit,
            other => {
                self.last_message = format!("key: {other:?}");
                Flow::Continue
            }
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        let body = Paragraph::new(vec![
            Line::from("ont — Vibe Machine ontology TUI"),
            Line::from(""),
            Line::from(format!("Version {VERSION}")),
            Line::from(""),
            Line::from("This is the v0 scaffold. Future panes:"),
            Line::from("  • corpus browser"),
            Line::from("  • run results"),
            Line::from("  • ontology explorer"),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" ont "));
        frame.render_widget(body, chunks[0]);

        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" v{VERSION} "),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("│ "),
            Span::raw(self.last_message.clone()),
            Span::raw("  "),
            Span::styled(HINT, Style::default().add_modifier(Modifier::DIM)),
        ]));
        frame.render_widget(status, chunks[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn pressing_q_exits() {
        let mut app = App::new();
        let flow = app.update(Message::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(matches!(flow, Flow::Exit));
    }

    #[test]
    fn ctrl_c_exits() {
        let mut app = App::new();
        let flow = app.update(Message::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(matches!(flow, Flow::Exit));
    }

    #[test]
    fn interrupt_message_exits() {
        let mut app = App::new();
        let flow = app.update(Message::Interrupt);
        assert!(matches!(flow, Flow::Exit));
    }

    #[test]
    fn unrelated_key_continues_and_records_label() {
        let mut app = App::new();
        let flow = app.update(Message::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
        )));
        assert!(matches!(flow, Flow::Continue));
        assert!(app.last_message.contains('x'));
    }
}
