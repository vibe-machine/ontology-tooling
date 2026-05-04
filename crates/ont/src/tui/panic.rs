use std::io;
use std::panic;

use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

/// Install a panic hook that restores the terminal before delegating to the
/// previously installed hook. Best-effort: errors during restore are ignored
/// because we are already panicking.
pub fn install_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}
