//! Relay server: forwards encrypted envelopes, never sees plaintext.

use anyhow::{Context, bail};
use termchat_relay::{Config, Limits, serve};

const USAGE: &str = "usage: termchat-relay --host <public host> [--listen 127.0.0.1:8080] [--trust-proxy] [--reports termchat-reports.log] [--db termchat-relay.db]";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    // The database, its WAL and the report journal must not be readable by others.
    #[cfg(unix)]
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));

    let mut listen = "127.0.0.1:8080".to_string();
    let mut host = None;
    let mut trust_proxy = false;
    let mut reports = "termchat-reports.log".to_string();
    let mut db = "termchat-relay.db".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().context(USAGE)?,
            "--host" => host = Some(args.next().context(USAGE)?),
            "--trust-proxy" => trust_proxy = true,
            "--reports" => reports = args.next().context(USAGE)?,
            "--db" => db = args.next().context(USAGE)?,
            _ => bail!(USAGE),
        }
    }
    let host = host.context(USAGE)?;

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!("listening on {listen} for {host}");
    serve(
        listener,
        Config {
            host,
            trust_proxy,
            db: Some(db.into()),
            reports: Some(reports.into()),
            limits: Limits::default(),
        },
    )
    .await?;
    Ok(())
}
