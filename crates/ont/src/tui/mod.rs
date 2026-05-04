use anyhow::Result;

use crate::cli::Cli;

mod app;
mod event;
mod panic;
mod terminal;

pub async fn run(_cli: &Cli) -> Result<()> {
    panic::install_hook();
    let mut session = terminal::TerminalSession::enter()?;
    let mut events = event::EventLoop::new()?;
    let mut app = app::App::new();

    loop {
        session.draw(|frame| app.draw(frame))?;
        let message = events.next().await?;
        if matches!(app.update(message), app::Flow::Exit) {
            break;
        }
    }
    Ok(())
}
