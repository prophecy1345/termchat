//! Terminal client (TUI) for termchat.

use std::io::stdout;
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TermEvent, EventStream, KeyEventKind,
};
use crossterm::terminal::SetTitle;
use futures_util::StreamExt;
use termchat::app::App;
use termchat::{data, ui};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("termchat {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    no_core_dumps();
    let path = data::home().join("state");
    // Alternate screen, raw mode, and a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    // Pasted text arrives as one event instead of key presses: otherwise control
    // bytes inside a pasted string would fire hotkeys (delete, report, wipe).
    let _ = crossterm::execute!(stdout(), EnableBracketedPaste);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
        hook(info);
    }));
    let result = run(&mut terminal, App::new(path)).await;
    let _ = crossterm::execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        SetTitle("")
    );
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> std::io::Result<()> {
    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut title = String::new();
    let mut mouse = false;
    while !app.quit {
        terminal.draw(|f| ui::render(f, &app))?;
        let now = app.title();
        if now != title {
            let _ = crossterm::execute!(stdout(), SetTitle(&now));
            title = now;
        }
        if app.settings.mouse != mouse {
            mouse = app.settings.mouse;
            let _ = match mouse {
                true => crossterm::execute!(stdout(), EnableMouseCapture),
                false => crossterm::execute!(stdout(), DisableMouseCapture),
            };
        }
        tokio::select! {
            ev = keys.next() => match ev {
                Some(Ok(TermEvent::Key(k))) if k.kind == KeyEventKind::Press => app.on_key(k),
                Some(Ok(TermEvent::Paste(text))) => app.on_paste(&text),
                Some(Ok(TermEvent::Mouse(e))) => app.on_mouse(e),
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
