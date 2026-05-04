use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CtEvent, EventStream, KeyEvent, MouseEvent};
use futures::StreamExt;
use tokio::signal::ctrl_c;
use tokio::time::{interval, Interval};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Message {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Tick,
    Interrupt,
}

pub struct EventLoop {
    crossterm: EventStream,
    ticks: Interval,
}

impl EventLoop {
    pub fn new() -> Result<Self> {
        Ok(Self {
            crossterm: EventStream::new(),
            ticks: interval(Duration::from_millis(250)),
        })
    }

    pub async fn next(&mut self) -> Result<Message> {
        tokio::select! {
            _ = ctrl_c() => Ok(Message::Interrupt),
            _ = self.ticks.tick() => Ok(Message::Tick),
            ct = self.crossterm.next() => {
                let event = match ct {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => return Err(error.into()),
                    None => return Ok(Message::Interrupt),
                };
                Ok(translate(event))
            }
        }
    }
}

fn translate(event: CtEvent) -> Message {
    match event {
        CtEvent::Key(key) => Message::Key(key),
        CtEvent::Mouse(mouse) => Message::Mouse(mouse),
        CtEvent::Resize(cols, rows) => Message::Resize(cols, rows),
        CtEvent::FocusGained | CtEvent::FocusLost | CtEvent::Paste(_) => Message::Tick,
    }
}
