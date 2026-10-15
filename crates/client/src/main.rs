//! Terminal client (TUI) for termchat.

use std::time::Duration;

use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures_util::StreamExt;
use termchat::app::App;
use termchat::{data, ui};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    no_core_dumps();
    let path = data::home().join("state");
    // Alternate screen, raw mode, and a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, App::new(path)).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> std::io::Result<()> {
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    while !app.quit {
        terminal.draw(|f| ui::render(f, &app))?;
        tokio::select! {
            ev = keys.next() => match ev {
                Some(Ok(TermEvent::Key(k))) if k.kind == KeyEventKind::Press => app.on_key(k),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e),
                None => return Ok(()),
            },
            ev = app.relay_event() => app.on_relay(ev),
            _ = tick.tick() => app.on_tick(),
        }
    }
    Ok(())
}

/// Keys must not end up in a core file after a crash.
fn no_core_dumps() {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, Rlimit, setrlimit};
        let _ = setrlimit(
            Resource::Core,
            Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        );
    }
}
