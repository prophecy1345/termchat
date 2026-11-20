use super::{
    Command, Duration, Instant, Output, PASSWORD, Path, Setup, Stdio, secret, start_relay_with, tmp,
};
use serde_json::Value;
use std::io::Write;
use termchat_engine::{Engine, Unlocked};
use termchat_relay::{Limits, Rate};

#[track_caller]
fn advance<'a>(
    peers: &'a mut [&mut Engine],
    mut done: impl FnMut(&mut [&mut Engine]) -> bool + 'a,
) -> impl Future<Output = ()> + 'a {
    let caller = std::panic::Location::caller();
    async move {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(peers) {
            assert!(
                Instant::now() < deadline,
                "group scenario timed out at {caller}"
            );
            for e in peers.iter_mut() {
                while let Ok(ev) = e.events.try_recv() {
                    e.on_relay(ev);
                }
                e.tick();
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}

fn cli(home: &Path, args: &[&str], input: Option<&str>) -> Child {
    let mut child = Child(Some(
        Command::new(env!("CARGO_BIN_EXE_termchat"))
            .env("TERMCHAT_HOME", home)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .unwrap(),
    ));
    if let Some(input) = input {
        child
            .0
            .as_mut()
            .unwrap()
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child
}

struct Child(Option<std::process::Child>);

impl Child {
    async fn output(mut self, peers: &mut [&mut Engine]) -> Output {
        advance(peers, |_| {
            self.0.as_mut().unwrap().try_wait().unwrap().is_some()
        })
        .await;
        self.0.take().unwrap().wait_with_output().unwrap()
    }

    async fn stop(self, peers: &mut [&mut Engine]) {
        assert!(
            Command::new("kill")
                .args(["-TERM", &self.0.as_ref().unwrap().id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        assert_code(self.output(peers).await, 0);
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn assert_code(output: Output, code: i32) -> String {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn rows(output: Output) -> Vec<Value> {
    assert_code(output, 0)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn send(e: &mut Engine, g: usize, text: &str) {
    e.send_group(g, text).unwrap();
}

fn open(home: &Path) -> Box<Engine> {
    let Unlocked::Open(engine) = Engine::unlock(&home.join("state"), PASSWORD.as_bytes()).unwrap()
    else {
        panic!("test identity was wiped");
    };
    engine
}

#[tokio::test(flavor = "multi_thread")]
async fn private_groups_through_cli_and_daemon() {
    let url = start_relay_with(Limits {
        queues_per_ip: Rate::per(100.0, 3600.0),
        ..Limits::default()
    })
    .await;
    let dir = tmp("groups");
    let pw = secret(&dir, PASSWORD, 0o600);
    let pw = pw.to_str().unwrap();
    let home = dir.join("server");
    let engine = |name: &str| {
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
    let (mut admin, mut server, mut recipient) =
        (engine("admin"), engine("server"), engine("team"));
    let (admin_key, server_key, recipient_key) = (admin.me(), server.me(), recipient.me());
    advance(&mut [&mut admin, &mut server, &mut recipient], |es| {
        es.iter()
            .all(|e| e.data.invite_queue.is_some() && e.data.fallback_published)
    })
    .await;
    for peer in [&mut server, &mut recipient] {
        let invite = peer.invite().unwrap();
        let key = peer.me();
        admin.add(&invite, false).unwrap();
        advance(&mut [&mut admin, peer], |es| es[1].is_request(&admin_key)).await;
        peer.accept(peer.index(&admin_key).unwrap());
        advance(&mut [&mut admin, peer], |es| {
            es[0].index(&key).is_some_and(|i| es[0].rich(i))
                && es[1].index(&admin_key).is_some_and(|i| es[1].rich(i))
        })
        .await;
    }
    // Also create a contact whose name matches both groups.
    let invite = recipient.invite().unwrap();
    server.add(&invite, false).unwrap();
    advance(&mut [&mut server, &mut recipient], |es| {
        es[1].is_request(&server_key)
    })
    .await;
    recipient.accept(recipient.index(&server_key).unwrap());
    advance(&mut [&mut server, &mut recipient], |es| {
        es[0].find("team").is_ok()
    })
    .await;

    let rejected = admin.create_group("unaccepted").unwrap();
    advance(&mut [&mut admin, &mut server], |es| {
        es[0].group_access(rejected)
    })
    .await;
    admin
        .invite_to_group(rejected, admin.index(&server_key).unwrap())
        .unwrap();
    advance(&mut [&mut admin, &mut server], |es| {
        !es[1].data.group_invites.is_empty()
    })
    .await;
    drop(server);
    let output = cli(
        &home,
        &["recv", "--group", "unaccepted", "--password-file", pw],
        None,
    )
    .output(&mut [&mut admin, &mut recipient])
    .await;
    assert_code(output, 3);
    let mut server = open(&home);
    server.connect_now().unwrap();
    server.decline_group(0);

    let mut ids = Vec::new();
    for name in ["team", "team"] {
        let g = admin.create_group(name).unwrap();
        let id = admin.data.groups[g].id;
        ids.push(id.iter().map(|b| format!("{b:02x}")).collect::<String>());
        advance(&mut [&mut admin, &mut server, &mut recipient], |es| {
            es[0].group_access(g)
        })
        .await;
        for key in [server_key, recipient_key] {
            admin
                .invite_to_group(g, admin.index(&key).unwrap())
                .unwrap();
        }
        advance(&mut [&mut admin, &mut server, &mut recipient], |es| {
            es[1..].iter().all(|e| {
                e.data
                    .group_invites
                    .iter()
                    .any(|inv| inv.roster.group == id)
            })
        })
        .await;
        for e in [&mut server, &mut recipient] {
            let at = e
                .data
                .group_invites
                .iter()
                .position(|inv| inv.roster.group == id)
                .unwrap();
            e.accept_group(at).unwrap();
        }
        advance(&mut [&mut admin, &mut server, &mut recipient], |es| {
            es.iter().all(|e| {
                e.group_index(&id)
                    .is_some_and(|i| e.group_access(i) && e.data.groups[i].members().len() == 3)
            })
        })
        .await;
    }
    let (first, second) = (ids[0].as_str(), ids[1].as_str());
    // A message produced before the upgrade already has the app's unread flag.
    send(&mut admin, 1, "legacy history");
    advance(&mut [&mut admin, &mut server, &mut recipient], |es| {
        es[1].data.groups[0]
            .messages
            .iter()
            .any(|m| m.text == "legacy history")
    })
    .await;
    let legacy = server.data.groups[0]
        .messages
        .iter_mut()
        .find(|m| m.text == "legacy history")
        .unwrap();
    assert!(legacy.unfetched);
    legacy.group_unfetched = false;
    assert!(server.persist());
    drop(server);
    let peers = &mut [&mut *admin, &mut *recipient];

    let listed = rows(
        cli(&home, &["groups", "--json", "--password-file", pw], None)
            .output(peers)
            .await,
    );
    assert_eq!(
        listed,
        vec![
            serde_json::json!({"id": first, "name": "team"}),
            serde_json::json!({"id": second, "name": "team"})
        ]
    );
    assert_code(
        cli(
            &home,
            &[
                "send",
                "--group",
                "team",
                "ambiguous",
                "--password-file",
                pw,
            ],
            None,
        )
        .output(peers)
        .await,
        3,
    );
    assert_code(
        cli(&home, &["recv", "--group", "team", "--from", "team"], None)
            .output(peers)
            .await,
        2,
    );
    assert_code(
        cli(&home, &["recv", "--group", "team", "--save-dir", "."], None)
            .output(peers)
            .await,
        2,
    );
    assert_code(
        cli(&home, &["recv", "--group-id", &first[..31]], None)
            .output(peers)
            .await,
        2,
    );
    assert_code(
        cli(
            &home,
            &["recv", "--group", "team", "--wait", "1", "--follow"],
            None,
        )
        .output(peers)
        .await,
        2,
    );

    let sent = cli(
        &home,
        &["send", "--group-id", first, "--password-file", pw],
        Some("direct from stdin\n"),
    );
    assert_code(sent.output(peers).await, 0);
    advance(peers, |es| {
        es[1].data.groups[0]
            .messages
            .iter()
            .any(|m| m.text == "direct from stdin")
    })
    .await;
    assert!(
        peers[0].data.groups[2]
            .messages
            .iter()
            .all(|m| m.text != "direct from stdin")
    );
    advance(peers, |es| {
        es[0].data.groups[1]
            .messages
            .iter()
            .any(|m| m.text == "direct from stdin")
    })
    .await;
    let at = peers[0].data.groups[1]
        .messages
        .iter()
        .position(|m| m.text == "direct from stdin")
        .unwrap();
    let reply_to = peers[0].data.groups[1].messages[at].id.unwrap().to_string();
    peers[0]
        .reply_group(1, at, "response\u{1b}\nnext line")
        .unwrap();
    let read = cli(
        &home,
        &[
            "recv",
            "--group-id",
            first,
            "--wait",
            "5",
            "--json",
            "--password-file",
            pw,
        ],
        None,
    );
    let records = rows(read.output(peers).await);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["text"], "response\nnext line");
    assert_eq!(records[0]["reply_to"], reply_to);
    assert!(records[0]["id"].is_string());
    assert_eq!(records[0]["group_id"], first);
    assert_eq!(
        records[0]["fingerprint"],
        termchat_core::fingerprint(&admin_key)
    );

    // Persist a queued text while the group's relay address is unreachable.
    let mut server = open(&home);
    server.data.groups[0].relay_url = "ws://127.0.0.1:0/".into();
    assert!(server.persist());
    drop(server);
    let out = cli(
        &home,
        &[
            "send",
            "--group-id",
            first,
            "after restart",
            "--timeout",
            "1",
            "--password-file",
            pw,
        ],
        None,
    )
    .output(peers)
    .await;
    assert_code(out, 4);
    let mut server = open(&home);
    server.data.groups[0].relay_url = url;
    assert!(server.persist());
    drop(server);

    let daemon = cli(&home, &["daemon", "--password-file", pw], None);
    advance(peers, |_| home.join("daemon.sock").exists()).await;
    advance(peers, |es| {
        es[1].data.groups[0]
            .messages
            .iter()
            .any(|m| m.text == "after restart")
    })
    .await;
    let late = cli(
        &home,
        &[
            "send",
            "--group-id",
            first,
            "late acceptance",
            "--timeout",
            "0",
        ],
        None,
    );
    assert_code(late.output(peers).await, 4);
    advance(peers, |es| {
        es[1].data.groups[0]
            .messages
            .iter()
            .any(|m| m.text == "late acceptance")
    })
    .await;

    let a = cli(
        &home,
        &["recv", "--group-id", first, "--wait", "5", "--json"],
        None,
    );
    let b = cli(
        &home,
        &["recv", "--group-id", first, "--wait", "5", "--json"],
        None,
    );
    let other = cli(
        &home,
        &["recv", "--group-id", second, "--wait", "5", "--json"],
        None,
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    // The waiters stay pinned while the signed group name changes.
    peers[0].data.groups[1].roster.as_mut().unwrap().name = "renamed".into();
    peers[0].set_group_timer(1, None).unwrap();
    send(peers[0], 1, "both readers");
    send(peers[0], 2, "other group");
    let to_server = peers[1].index(&server_key).unwrap();
    peers[1].send(to_server, "direct only").unwrap();
    let delivered = rows(a.output(peers).await);
    assert_eq!(delivered, rows(b.output(peers).await));
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["group"], "renamed");
    assert_eq!(delivered[0]["text"], "both readers");
    assert_eq!(rows(other.output(peers).await)[0]["text"], "other group");
    assert_eq!(
        rows(
            cli(
                &home,
                &["recv", "--from", "team", "--wait", "5", "--json"],
                None
            )
            .output(peers)
            .await
        )[0]["text"],
        "direct only"
    );
    assert!(
        rows(
            cli(&home, &["recv", "--group", "renamed", "--json"], None)
                .output(peers)
                .await
        )
        .is_empty()
    );
    assert_code(
        cli(
            &home,
            &["send", "--group", "team"],
            Some("daemon from stdin\n"),
        )
        .output(peers)
        .await,
        0,
    );
    advance(peers, |es| {
        es[1].data.groups[1]
            .messages
            .iter()
            .any(|m| m.text == "daemon from stdin")
    })
    .await;

    let mut broken = cli(
        &home,
        &["recv", "--group-id", second, "--follow", "--json"],
        None,
    );
    drop(broken.0.as_mut().unwrap().stdout.take());
    tokio::time::sleep(Duration::from_millis(350)).await;
    send(peers[0], 2, "broken pipe");
    assert_code(broken.output(peers).await, 1);

    let follow = cli(
        &home,
        &["recv", "--group-id", first, "--follow", "--json"],
        None,
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    peers[0].remove_from_group(1, &server_key).unwrap();
    assert_code(follow.output(peers).await, 5);
    assert_code(
        cli(&home, &["send", "--group-id", first, "removed"], None)
            .output(peers)
            .await,
        5,
    );
    daemon.stop(peers).await;

    // An invalidated send queue is a real relay refusal, even with time left.
    let mut server = open(&home);
    server.data.groups[1].send_id = [0xff; 16];
    assert!(server.persist());
    drop(server);
    let started = Instant::now();
    let refused = cli(
        &home,
        &[
            "send",
            "--group-id",
            second,
            "rejected",
            "--timeout",
            "10",
            "--password-file",
            pw,
        ],
        None,
    );
    assert_code(refused.output(peers).await, 5);
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
}
