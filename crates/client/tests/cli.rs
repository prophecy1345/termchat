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

#[cfg(unix)]
#[path = "cli/groups.rs"]
mod groups;

async fn start_relay() -> String {
    start_relay_with(Limits::default()).await
}

async fn start_relay_with(limits: Limits) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let config = Config {
        trust_proxy: false,
        db: None,
        reports: None,
        limits,
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

async fn pump(phone: &mut Engine, mut done: impl FnMut(&Engine) -> bool) {
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ask_accepts_only_the_current_reply_from_the_verified_contact() {
    use std::io::{BufRead, BufReader, Read};

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl Child {
        async fn result(&mut self, phone: &mut Engine) -> (Option<i32>, String, String) {
            pump(phone, |_| self.0.try_wait().unwrap().is_some()).await;
            let code = self.0.wait().unwrap().code();
            let (mut out, mut err) = (String::new(), String::new());
            self.0
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut out)
                .unwrap();
            self.0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut err)
                .unwrap();
            (code, out, err)
        }
    }

    async fn question(phone: &mut Engine, i: usize, topic: &str) -> (usize, String) {
        let prefix = format!("{topic}\n\n");
        let find = |e: &Engine| {
            e.data.contacts[i]
                .messages
                .iter()
                .position(|m| !m.mine && m.text.starts_with(&prefix))
        };
        pump(phone, |e| find(e).is_some()).await;
        let m = find(phone).unwrap();
        let text = &phone.data.contacts[i].messages[m].text;
        let (_, yes) = text
            .lines()
            .find_map(|l| l.split_once("To approve, send: "))
            .unwrap();
        let code = yes.strip_prefix("yes ").unwrap();
        assert_eq!(code.len(), 16);
        assert!(text.ends_with(&format!("To decline, send: no {code}")));
        (m, code.to_owned())
    }

    let url = start_relay().await;
    let dir = tmp("ask");
    let pw = secret(&dir, PASSWORD, 0o600);
    let engine = |name| {
        Engine::setup(
            &dir.join(name).join("state"),
            &Setup {
                relay_url: &url,
                onion_url: None,
                tor: false,
                nickname: name,
                password: PASSWORD,
            },
        )
        .unwrap()
    };
    let (mut box_, mut alice, mut other) = (engine("box"), engine("alice"), engine("other"));
    pair(&mut box_, &mut alice).await;
    pair(&mut box_, &mut other).await;
    let alice_chat = alice.index(&box_.me()).unwrap();
    let other_chat = other.index(&box_.me()).unwrap();
    drop(box_);
    let home = dir.join("box");
    let start = |args: &[&str]| {
        Child(
            Command::new(env!("CARGO_BIN_EXE_termchat"))
                .env("TERMCHAT_HOME", &home)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    };
    let daemon_args = ["daemon", "--password-file", pw.to_str().unwrap()];
    let ask = |topic, timeout| start(&["ask", "alice", topic, "--timeout", timeout]);
    assert_eq!(
        text(&termchat(&mut alice, &home, &["ask", "alice", "Hi"]).await).0,
        Some(6)
    );
    for timeout in [
        "0",
        "0m",
        "-1",
        "+1",
        "1.2m",
        "18446744073709551615",
        "18446744073709551615m",
    ] {
        let result = termchat(
            &mut alice,
            &home,
            &["ask", "alice", "Hi", "--timeout", timeout],
        )
        .await;
        assert_eq!(result.status.code(), Some(2), "{timeout}");
    }
    let private = "--private question";
    let result = termchat(&mut alice, &home, &["ask", "alice", private]).await;
    assert_eq!(result.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(private));

    let mut daemon = start(&daemon_args);
    pump(&mut alice, |_| home.join("daemon.sock").exists()).await;
    let mut follower = start(&["recv", "--follow", "--json"]);
    let output = BufReader::new(follower.0.stdout.take().unwrap());
    let (seen, mut messages) = tokio::sync::mpsc::unbounded_channel();
    let read = tokio::task::spawn_blocking(move || {
        for line in output.lines().map_while(Result::ok) {
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            if let Some(text) = value["text"].as_str()
                && seen.send(text.to_owned()).is_err()
            {
                break;
            }
        }
    });
    let mut first = start(&[
        "ask",
        "alice",
        "--timeout",
        "10m",
        "--",
        "--Deploy build 42?",
    ]);
    let (m, code) = question(&mut alice, alice_chat, "--Deploy build 42?").await;
    let yes = format!("yes {code}");
    // ADR-034: case and edge whitespace are fine, an extra word is not.
    let malformed = format!("{yes} please");
    assert_eq!(
        text(&termchat(&mut alice, &home, &["ask", "missing", "Hi"]).await).0,
        Some(3)
    );

    other.send(other_chat, &yes).unwrap();
    pump(&mut other, Engine::all_answered).await;
    alice.send(alice_chat, &malformed).unwrap();
    pump(&mut alice, Engine::all_answered).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut wrong_contact, mut wrong_text) = (false, false);
        while !(wrong_contact && wrong_text) {
            let reply = messages.recv().await.unwrap();
            wrong_contact |= reply == yes;
            wrong_text |= reply == malformed;
        }
    })
    .await
    .unwrap();

    let busy = text(&termchat(&mut alice, &home, &["ask", "alice", "Collision?"]).await);
    assert_eq!(busy.0, Some(5));
    assert!(busy.2.contains("already pending"));
    alice.reply(alice_chat, m, &yes).unwrap();
    assert_eq!(
        first.result(&mut alice).await,
        (Some(0), String::new(), String::new())
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while messages.recv().await.unwrap() != yes {}
    })
    .await
    .unwrap();
    drop(follower);
    read.await.unwrap();

    let mut timed = ask("Time out?", "2");
    let (_, expired) = question(&mut alice, alice_chat, "Time out?").await;
    let (status, stdout, stderr) = timed.result(&mut alice).await;
    assert_eq!(status, Some(4));
    assert!(stdout.is_empty() && !stderr.contains(&expired) && !stderr.contains("Time out?"));

    let mut interrupted = ask("Lose daemon?", "10m");
    let (_, old) = question(&mut alice, alice_chat, "Lose daemon?").await;
    let kill = Command::new("kill")
        .args(["-TERM", &daemon.0.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    assert_eq!(interrupted.result(&mut alice).await.0, Some(6));
    assert_eq!(daemon.result(&mut alice).await.0, Some(0));

    let daemon = start(&daemon_args);
    pump(&mut alice, |_| home.join("daemon.sock").exists()).await;
    let mut last = ask("Deploy build 43?", "10m");
    let (_, code) = question(&mut alice, alice_chat, "Deploy build 43?").await;
    assert_ne!(code, old);
    assert_ne!(code, expired);
    for stale in [old, expired] {
        alice.send(alice_chat, &format!("yes {stale}")).unwrap();
    }
    alice.send(alice_chat, &format!("no {code}")).unwrap();
    let (status, stdout, stderr) = last.result(&mut alice).await;
    assert_eq!(status, Some(1));
    assert!(stdout.is_empty() && !stderr.contains(&code) && !stderr.contains("Deploy build 43?"));

    let mut departed = ask("Contact leaves?", "10m");
    question(&mut alice, alice_chat, "Contact leaves?").await;
    assert!(alice.leave(false));
    assert_eq!(departed.result(&mut alice).await.0, Some(5));
    drop(daemon);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
async fn pair(a: &mut Engine, b: &mut Engine) {
    let (a_key, b_key) = (a.me(), b.me());
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut added = false;
    loop {
        if !added && b.data.fallback_published && b.all_connected() {
            a.add(&b.invite().unwrap(), false).unwrap();
            added = true;
        }
        if let Some(i) = b.index(&a_key)
            && b.is_request(&a_key)
            && b.data.contacts[i].theirs.is_some()
        {
            b.accept(i);
        }
        if let (Some(i), Some(j)) = (a.index(&b_key), b.index(&a_key))
            && a.rich(i)
            && b.rich(j)
            && a.all_answered()
            && b.all_answered()
        {
            a.edit(i, |c| c.verified = true);
            b.edit(j, |c| c.verified = true);
            return;
        }
        assert!(Instant::now() < deadline, "contact pairing timed out");
        tokio::select! {
            Some(ev) = a.events.recv() => a.on_relay(ev),
            Some(ev) = b.events.recv() => b.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        a.tick();
        b.tick();
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
        vec!["run", "--", "true"],
        vec!["run", "--to", "a", "--group", "b", "--", "true"],
        vec!["run", "--to", "a", "--wait", "5", "--", "true"],
        vec!["run", "--to", "a", "--lines", "51", "--", "true"],
        vec!["run", "--to", "a", "--password-stdin", "--", "true"],
        vec!["run", "--to", "a"],
        vec!["send", "alice", "hi", "--failures-only"],
        vec!["send", "alice", "hi", "--expires", "5s"],
        vec!["send", "alice", "hi", "--expires", "8d"],
        vec!["send", "alice", "hi", "--expires", "5x"],
        vec!["send", "alice", "hi", "--expires", "-5m"],
        vec!["send", "--group", "ops", "hi", "--expires", "5m"],
        vec!["recv", "--expires", "5m"],
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

/// ADR-029: with `termchat daemon` running, the same commands work without
/// a password, documents included, and a follower sees new messages.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn commands_go_through_a_running_daemon() {
    use std::io::BufRead;
    use std::os::unix::fs::PermissionsExt;

    let url = start_relay().await;
    let dir = tmp("daemon");
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
    let init = [
        "init",
        "--nickname",
        "box",
        "--relay",
        &url,
        "--password-file",
        pw,
    ];
    assert_eq!(
        termchat(&mut phone, &home, &init).await.status.code(),
        Some(0)
    );
    let add = ["add", &invite, "--password-file", pw];
    assert_eq!(
        termchat(&mut phone, &home, &add).await.status.code(),
        Some(0)
    );
    pump(&mut phone, |e| {
        e.data.contacts.first().is_some_and(|c| c.theirs.is_some())
    })
    .await;
    phone.accept(0);
    pump(&mut phone, |e| !e.data.contacts[0].accept_pending).await;

    /// Killed if the test fails halfway, so no daemon outlives it.
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_termchat"))
            .env("TERMCHAT_HOME", &home)
            .args(["daemon", "--password-file", pw])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = home.join("daemon.sock");
    pump(&mut phone, |_| socket.exists()).await;
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // The socket comes before the daemon has read its queues: her name
    // arrives with the accept it fetches. A script waits the same way.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (_, out, _) = text(&termchat(&mut phone, &home, &["contacts"]).await);
        if out.starts_with("alice") {
            break;
        }
        assert!(Instant::now() < deadline, "the daemon never heard alice");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // No password from here on.
    let started = Instant::now();
    let (code, _, err) = text(&termchat(&mut phone, &home, &["send", "alice", "disk full"]).await);
    assert_eq!(code, Some(0), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "no new connection"
    );
    pump(&mut phone, |e| {
        e.data.contacts[0]
            .messages
            .iter()
            .any(|m| m.text == "disk full")
    })
    .await;
    let (code, out, _) = text(&termchat(&mut phone, &home, &["contacts"]).await);
    assert_eq!(code, Some(0));
    assert!(out.contains("\tok"), "{out}");
    let (code, _, _) = text(&termchat(&mut phone, &home, &["send", "nobody", "hi"]).await);
    assert_eq!(code, Some(3));

    // A follower sees a text from the phone.
    let mut follower = Command::new(env!("CARGO_BIN_EXE_termchat"))
        .env("TERMCHAT_HOME", &home)
        .args(["recv", "--follow", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = std::io::BufReader::new(follower.stdout.take().unwrap());
    // ADR-041: the phone's receipt for "disk full" may come first.
    let mut first = tokio::task::spawn_blocking(move || {
        lines
            .lines()
            .map(Result::unwrap)
            .find(|l| l.contains("\"kind\":\"message\""))
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    phone.send(0, "status?").unwrap();
    let first = loop {
        tokio::select! {
            line = &mut first => break line.unwrap(),
            Some(ev) = phone.events.recv() => phone.on_relay(ev),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        phone.tick();
    };
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["text"], "status?");
    follower.kill().unwrap();
    let _ = follower.wait();

    // A document, saved through the daemon.
    phone
        .send_file(0, "notes.txt", b"all good".to_vec())
        .unwrap();
    let saved = dir.join("saved");
    std::fs::create_dir_all(&saved).unwrap();
    let save = [
        "recv",
        "--json",
        "--wait",
        "15",
        "--save-dir",
        saved.to_str().unwrap(),
    ];
    let (code, out, err) = text(&termchat(&mut phone, &home, &save).await);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("file saved"), "{out}");
    assert_eq!(std::fs::read(saved.join("notes.txt")).unwrap(), b"all good");

    // SIGTERM: the daemon saves, removes its socket and exits 0.
    let killed = Command::new("kill")
        .args(["-TERM", &daemon.0.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    assert_eq!(daemon.0.wait().unwrap().code(), Some(0));
    assert!(!socket.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn watch_alerts_the_owner_once_and_rearms_after_a_pulse() {
    use std::io::{BufRead, BufReader};

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn start(home: &Path, args: &[&str]) -> Child {
        Child(
            Command::new(env!("CARGO_BIN_EXE_termchat"))
                .env("TERMCHAT_HOME", home)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
    async fn ready(child: &mut Child) {
        let output = BufReader::new(child.0.stdout.take().unwrap());
        let task = tokio::task::spawn_blocking(move || output.lines().next().unwrap().unwrap());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap(),
            "watch ready"
        );
    }

    let url = start_relay().await;
    let dir = tmp("watch");
    let pw = secret(&dir, PASSWORD, 0o600);
    let engine = |name| {
        Engine::setup(
            &dir.join(name).join("state"),
            &Setup {
                relay_url: &url,
                onion_url: None,
                tor: false,
                nickname: name,
                password: PASSWORD,
            },
        )
        .unwrap()
    };
    let (mut server, mut watcher, mut owner) = (engine("box"), engine("watcher"), engine("owner"));
    pair(&mut server, &mut watcher).await;
    pair(&mut watcher, &mut owner).await;
    let watcher_key = watcher.me();
    drop((server, watcher));

    let sender_home = dir.join("box");
    let watcher_home = dir.join("watcher");
    let send = ["watch", "send", "watcher", "--every", "1"];
    let no_daemon = termchat(&mut owner, &sender_home, &send).await;
    assert_eq!(no_daemon.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&no_daemon.stderr).contains("needs a running"));
    let daemon = ["daemon", "--password-file", pw.to_str().unwrap()];
    let mut sender_daemon = start(&sender_home, &daemon);
    let watcher_daemon = start(&watcher_home, &daemon);
    pump(&mut owner, |_| {
        sender_home.join("daemon.sock").exists() && watcher_home.join("daemon.sock").exists()
    })
    .await;

    let mut sender = start(&sender_home, &send);
    ready(&mut sender).await;
    let mut monitor = start(
        &watcher_home,
        &[
            "watch", "monitor", "box", "--notify", "owner", "--after", "4",
        ],
    );
    ready(&mut monitor).await;
    let alerts = |e: &Engine| {
        e.data.contacts[0]
            .messages
            .iter()
            .filter(|m| m.text == "box has been silent for 4 seconds")
            .count()
    };
    let after = |seconds| Instant::now() + Duration::from_secs(seconds);
    let until = after(5);
    pump(&mut owner, |_| Instant::now() >= until).await;
    assert_eq!(alerts(&owner), 0, "live pulses must hold the deadline");
    let recv = termchat(
        &mut owner,
        &watcher_home,
        &["recv", "--from", "box", "--json"],
    )
    .await;
    assert!(
        recv.status.success() && recv.stdout.is_empty(),
        "pulses must be hidden from recv: {:?}",
        text(&recv)
    );

    drop(sender);
    let limit = after(6);
    while alerts(&owner) == 0 && Instant::now() < limit {
        owner.send_heartbeat(&watcher_key).unwrap();
        let until = after(1);
        pump(&mut owner, |e| alerts(e) > 0 || Instant::now() >= until).await;
    }
    assert_eq!(
        alerts(&owner),
        1,
        "pulses from the owner must not hide an outage"
    );
    let until = after(5);
    pump(&mut owner, |_| Instant::now() >= until).await;
    assert_eq!(alerts(&owner), 1, "the same outage is reported once");

    let mut sender = start(&sender_home, &send);
    ready(&mut sender).await;
    let until = after(2);
    pump(&mut owner, |_| Instant::now() >= until).await;
    sender_daemon.0.kill().unwrap();
    let _ = sender_daemon.0.wait();
    assert_eq!(
        sender.0.wait().unwrap().code(),
        Some(1),
        "loss of the daemon must stop watch send"
    );
    pump(&mut owner, |e| alerts(e) == 2).await;
    drop((monitor, watcher_daemon));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn run_reports_how_a_command_ended() {
    let url = start_relay().await;
    let dir = tmp("run");
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
    let init = [
        "init",
        "--nickname",
        "box",
        "--relay",
        &url,
        "--password-file",
        pw,
    ];
    assert_eq!(
        termchat(&mut phone, &home, &init).await.status.code(),
        Some(0)
    );
    let add = ["add", &invite, "--password-file", pw];
    assert_eq!(
        termchat(&mut phone, &home, &add).await.status.code(),
        Some(0)
    );
    pump(&mut phone, |e| {
        e.data.contacts.first().is_some_and(|c| c.theirs.is_some())
    })
    .await;
    phone.accept(0);
    pump(&mut phone, |e| !e.data.contacts[0].accept_pending).await;
    let reports = |e: &Engine| -> Vec<String> {
        e.data.contacts[0]
            .messages
            .iter()
            .filter(|m| {
                !m.mine
                    && m.status != termchat_engine::data::Status::System
                    && m.expires_at.is_none()
            })
            .map(|m| m.text.clone())
            .collect()
    };

    // Without a daemon: the command's status, and one report with its output.
    let failing = [
        "run",
        "--to",
        "alice",
        "--password-file",
        pw,
        "--",
        "sh",
        "-c",
        "echo hello; echo token=s3cret-arg >/dev/null; exit 3",
    ];
    let (code, out, _) = text(&termchat(&mut phone, &home, &failing).await);
    assert_eq!(code, Some(3));
    assert_eq!(out, "hello\n", "output still reaches the caller");
    pump(&mut phone, |e| !reports(e).is_empty()).await;
    let report = &reports(&phone)[0];
    assert!(
        report.starts_with("sh failed with exit 3 after "),
        "{report}"
    );
    assert!(report.ends_with("\nhello"), "{report}");
    assert!(
        !report.contains("s3cret"),
        "arguments stay out of the report"
    );

    // A recipient that cannot be written to stops run before the command.
    let marker = dir.join("ran");
    let marker = marker.to_str().unwrap();
    let unknown = [
        "run",
        "--to",
        "nobody",
        "--password-file",
        pw,
        "--",
        "touch",
        marker,
    ];
    assert_eq!(
        termchat(&mut phone, &home, &unknown).await.status.code(),
        Some(3)
    );
    let no_password = ["run", "--to", "alice", "--", "touch", marker];
    assert_eq!(
        termchat(&mut phone, &home, &no_password)
            .await
            .status
            .code(),
        Some(2)
    );
    assert!(!Path::new(marker).exists(), "the command must not start");

    // Success is quiet with --failures-only, and named with --name.
    let quiet = [
        "run",
        "--to",
        "alice",
        "--failures-only",
        "--password-file",
        pw,
        "--",
        "true",
    ];
    assert_eq!(
        termchat(&mut phone, &home, &quiet).await.status.code(),
        Some(0)
    );
    let named = [
        "run",
        "--to",
        "alice",
        "--name",
        "nightly",
        "--password-file",
        pw,
        "--",
        "true",
    ];
    assert_eq!(
        termchat(&mut phone, &home, &named).await.status.code(),
        Some(0)
    );
    pump(&mut phone, |e| reports(e).len() == 2).await;
    assert!(reports(&phone)[1].starts_with("nightly finished after "));

    // send --expires: the message carries its own timer, with or without a daemon.
    let expiring = |e: &Engine, text: &str| {
        e.data.contacts[0]
            .messages
            .iter()
            .find(|m| m.text == text)
            .and_then(|m| m.expires_at.map(|end| end - m.at))
    };
    let secret = [
        "send",
        "alice",
        "pin 1234",
        "--expires",
        "5m",
        "--password-file",
        pw,
    ];
    assert_eq!(
        termchat(&mut phone, &home, &secret).await.status.code(),
        Some(0)
    );
    pump(&mut phone, |e| expiring(e, "pin 1234").is_some()).await;
    assert_eq!(expiring(&phone, "pin 1234"), Some(300));

    // Through a daemon: no password, and a program that is not there.
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_termchat"))
            .env("TERMCHAT_HOME", &home)
            .args(["daemon", "--password-file", pw])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = home.join("daemon.sock");
    pump(&mut phone, |_| socket.exists()).await;
    let token = ["send", "alice", "token", "--expires", "30s"];
    assert_eq!(
        termchat(&mut phone, &home, &token).await.status.code(),
        Some(0)
    );
    pump(&mut phone, |e| expiring(e, "token").is_some()).await;
    assert_eq!(expiring(&phone, "token"), Some(30));
    let missing = [
        "run",
        "--to",
        "alice",
        "--lines",
        "0",
        "--",
        "/nonexistent/job",
    ];
    assert_eq!(
        termchat(&mut phone, &home, &missing).await.status.code(),
        Some(127)
    );
    pump(&mut phone, |e| reports(e).len() == 3).await;
    let report = &reports(&phone)[2];
    assert!(report.starts_with("job could not start: "), "{report}");
    assert_eq!(report.lines().count(), 1, "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}
