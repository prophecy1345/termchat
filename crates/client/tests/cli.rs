//! The scriptable mode end to end (ADR-024): a "server" identity driven
//! through the real `termchat` binary, and a "phone" engine in the test.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-only file

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use termchat_engine::{Engine, Setup};
use termchat_relay::{Config, Limits};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery staple violin";

async fn start_relay() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
        limits: Limits::default(),
    };
    tokio::spawn(termchat_relay::serve(listener, None, config));
    url
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("termchat-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn secret(dir: &Path, text: &str, mode: u32) -> PathBuf {
    let path = dir.join("pw");
    std::fs::write(&path, format!("{text}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    path
}

/// Runs `termchat ARGS` for the identity in `home`, while `phone` keeps
/// talking to the relay.
async fn termchat(phone: &mut Engine, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_termchat"));
    cmd.env("TERMCHAT_HOME", home)
        .args(args)
        .stdin(Stdio::null());
    let run = tokio::task::spawn_blocking(move || cmd.output().unwrap());
    tokio::pin!(run);
    loop {
        tokio::select! {
            out = &mut run => return out.unwrap(),
            Some(ev) = phone.events.recv() => phone.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        phone.tick();
    }
}

async fn pump(phone: &mut Engine, done: impl Fn(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(phone) {
        assert!(Instant::now() < deadline, "timed out: {:?}", phone.happened);
        tokio::select! {
            Some(ev) = phone.events.recv() => phone.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        phone.tick();
    }
}

fn text(out: &Output) -> (Option<i32>, String, String) {
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// An invite printed by `termchat invite` works while that identity is
/// offline: its keys went out before the command ended.
#[tokio::test(flavor = "multi_thread")]
async fn an_invite_from_the_command_line_works_offline() {
    let url = start_relay().await;
    let dir = tmp("offline");
    let pw = secret(&dir, PASSWORD, 0o600);
    let home = dir.clone();
    let run = move |name: &str, args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_termchat"));
        cmd.env("TERMCHAT_HOME", home.join(name))
            .args(args)
            .args(["--password-file", pw.to_str().unwrap()])
            .stdin(Stdio::null());
        text(&cmd.output().unwrap())
    };
    let url = url.clone();
    let out = tokio::task::spawn_blocking(move || {
        for name in ["one", "two"] {
            assert_eq!(
                run(name, &["init", "--nickname", name, "--relay", &url]).0,
                Some(0)
            );
        }
        let (code, invite, _) = run("one", &["invite"]);
        assert_eq!(code, Some(0));
        run("two", &["add", invite.trim(), "--timeout", "15"])
    })
    .await
    .unwrap();
    assert_eq!(out.0, Some(0), "{out:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_identity_sends_and_receives_from_the_command_line() {
    let url = start_relay().await;
    let dir = tmp("e2e");
    let setup = Setup {
        relay_url: &url,
        onion_url: None,
        tor: false,
        nickname: "alice",
        password: PASSWORD,
    };
    let mut phone = Engine::setup(&dir.join("phone"), &setup).unwrap();
    pump(&mut phone, |e| e.data.fallback_published).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let invite = phone.invite().unwrap();

    let home = dir.join("server");
    std::fs::create_dir_all(&home).unwrap();
    let pw = secret(&dir, PASSWORD, 0o600);
    let pw = pw.to_str().unwrap();
    let (code, out, err) = text(
        &termchat(
            &mut phone,
            &home,
            &[
                "init",
                "--nickname",
                "backup-box",
                "--relay",
                &url,
                "--password-file",
                pw,
            ],
        )
        .await,
    );
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("fingerprint"), "{out}");

    let (code, out, err) =
        text(&termchat(&mut phone, &home, &["add", &invite, "--password-file", pw]).await);
    assert_eq!(code, Some(0), "{err}");
    // Their name comes with their acceptance; until then, their fingerprint.
    let fp = termchat_core::fingerprint(&phone.me());
    assert!(
        out.contains(&format!("request sent to {}", &fp[..5])),
        "{out}"
    );
    pump(&mut phone, |e| {
        e.data.contacts.first().is_some_and(|c| c.theirs.is_some())
    })
    .await;
    phone.accept(0);
    pump(&mut phone, |e| !e.data.contacts[0].accept_pending).await;

    // The server writes; the phone reads.
    let (code, _, err) = text(
        &termchat(
            &mut phone,
            &home,
            &["send", "alice", "backup failed", "--password-file", pw],
        )
        .await,
    );
    assert_eq!(code, Some(0), "{err}");
    pump(&mut phone, |e| {
        e.data.contacts[0]
            .messages
            .iter()
            .any(|m| m.text == "backup failed")
    })
    .await;

    // The phone answers, with an escape sequence that must not reach a terminal.
    phone.send(0, "yes, restart\u{1b}[2J").unwrap();
    pump(&mut phone, |e| {
        e.data.contacts[0]
            .messages
            .last()
            .is_some_and(|m| matches!(m.status, termchat_engine::data::Status::Sent))
    })
    .await;
    let (code, out, err) = text(
        &termchat(
            &mut phone,
            &home,
            &["recv", "--json", "--wait", "15", "--password-file", pw],
        )
        .await,
    );
    assert_eq!(code, Some(0), "{err}");
    let line: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(
        line["from"],
        "alice·".to_string() + &termchat_core::fingerprint(&phone.me())[..5]
    );
    assert_eq!(line["text"], "yes, restart[2J");
    assert!(!out.contains('\u{1b}'), "no escape sequence in the output");
    let (code, out, _) = text(&termchat(&mut phone, &home, &["recv", "--password-file", pw]).await);
    assert_eq!((code, out.as_str()), (Some(0), ""), "each message once");

    // Wrong contact, wrong password, a password file others can read.
    let (code, _, err) = text(
        &termchat(
            &mut phone,
            &home,
            &["send", "nobody", "hi", "--password-file", pw],
        )
        .await,
    );
    assert_eq!(code, Some(3), "{err}");
    let wrong = secret(&dir, "not the password at all", 0o600);
    let (code, _, err) = text(
        &termchat(
            &mut phone,
            &home,
            &["contacts", "--password-file", wrong.to_str().unwrap()],
        )
        .await,
    );
    assert_eq!(code, Some(1), "{err}");
    #[cfg(unix)]
    {
        let open = secret(&dir, PASSWORD, 0o644);
        let (code, _, err) = text(
            &termchat(
                &mut phone,
                &home,
                &["contacts", "--password-file", open.to_str().unwrap()],
            )
            .await,
        );
        assert_eq!(code, Some(1));
        assert!(err.contains("chmod 600"), "{err}");
    }

    // The identity is open elsewhere: say so at once.
    let pw = secret(&dir, PASSWORD, 0o600);
    let held = termchat_engine::data::lock(&home.join("state")).unwrap();
    let started = Instant::now();
    let (code, _, err) = text(
        &termchat(
            &mut phone,
            &home,
            &["contacts", "--password-file", pw.to_str().unwrap()],
        )
        .await,
    );
    assert_eq!(code, Some(1));
    assert!(err.contains("open in another termchat"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5), "does not wait");
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_arguments_exit_with_2() {
    let home = tmp("args");
    for args in [
        vec!["send"],
        vec!["recv", "--wait", "soon"],
        vec!["init"],
        vec!["contacts", "--password-file", "a", "--password-stdin"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_termchat"))
            .env("TERMCHAT_HOME", &home)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&home);
}
