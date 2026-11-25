//! Daemon mode (ADR-029): `termchat daemon` keeps one identity open and
//! answers the scriptable commands over a Unix socket in the state folder.
//! One JSON object per line each way; only the daemon's own user gets in.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use termchat_core::proto::GroupId;
use termchat_engine::Engine;
use termchat_engine::data::{self, Status};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::{
    Args, Fail, Mark, USAGE, can_write, contact_rows, fail, group, last_mark, print_record, record,
    status_of,
};

mod approval;
mod groups;
pub(super) mod paging;
mod watch;

pub const SOCKET: &str = "daemon.sock";
/// A 1 MB document in base64, with room for the rest of the line.
const MAX_LINE: u64 = 2 << 20;
const MAX_CONNECTIONS: usize = 16;

/// A request and where its answer lines go.
enum Job {
    Command(Value, mpsc::UnboundedSender<String>),
    GroupReader(Value, mpsc::Sender<String>, oneshot::Sender<String>),
}

/// Something waiting on the relay: a sent message, or a request out.
enum Waiting {
    Sent { i: usize, m: Mark },
    GroupSent { group: GroupId, id: u64 },
    Added { i: usize },
}

struct Pending {
    what: Waiting,
    until: Instant,
    reply: mpsc::UnboundedSender<String>,
}

/// A `recv` that waits or follows.
struct Reader {
    from: Option<usize>,
    files: bool,
    follow: bool,
    until: Option<Instant>,
    got: usize,
    reply: mpsc::UnboundedSender<String>,
}

fn ok(more: Value) -> String {
    let mut v = json!({ "ok": true });
    if let (Some(v), Some(more)) = (v.as_object_mut(), more.as_object()) {
        v.extend(more.clone());
    }
    v.to_string()
}

fn err(Fail(code, why): Fail) -> String {
    json!({ "ok": false, "code": code, "error": why }).to_string()
}

pub(super) async fn serve(e: &mut Engine, home: &Path) -> Result<(), Fail> {
    let path = home.join(SOCKET);
    // We hold the state lock, so a socket here is a crashed daemon's.
    let _ = std::fs::remove_file(&path);
    let listener =
        UnixListener::bind(&path).map_err(|why| Fail(1, format!("cannot listen: {why}")))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|why| Fail(1, format!("cannot protect the socket: {why}")))?;
    }
    use tokio::signal::unix::{SignalKind, signal};
    let signals = |k| signal(k).map_err(|why| Fail(1, format!("signals: {why}")));
    let (mut term, mut int) = (
        signals(SignalKind::terminate())?,
        signals(SignalKind::interrupt())?,
    );
    let me = rustix::process::geteuid().as_raw();
    let (tx, mut rx) = mpsc::channel::<Job>(64);
    let open = Arc::new(AtomicUsize::new(0));
    let (mut pending, mut readers) = (Vec::new(), Vec::new());
    let mut group_readers = Vec::new();
    let mut watches = Vec::new();
    let mut approvals = Vec::new();
    let mut pages = Vec::new();
    eprintln!("termchat: daemon ready on {}", path.display());
    loop {
        let mut answer = None;
        let mut ack = None;
        tokio::select! {
            Ok((stream, _)) = listener.accept() => {
                // Only our own user; anyone else is closed without a word.
                let ours = stream.peer_cred().is_ok_and(|c| c.uid() == me);
                if ours && open.fetch_add(1, Ordering::SeqCst) < MAX_CONNECTIONS {
                    let (tx, open) = (tx.clone(), open.clone());
                    tokio::spawn(async move {
                        connection(stream, tx).await;
                        open.fetch_sub(1, Ordering::SeqCst);
                    });
                } else if ours {
                    open.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Some(job) = rx.recv() => {
                match job {
                    Job::GroupReader(request, records, done) => {
                        groups::register(e, request, records, done, &mut group_readers);
                    }
                    Job::Command(request, reply) if watch::is_request(&request) => {
                        watch::register(e, &request, reply, &mut watches);
                    }
                    Job::Command(request, reply) if request["cmd"] == "ask" => {
                        approval::register(e, &request, reply, &mut approvals);
                    }
                    Job::Command(request, reply) if paging::is_request(&request) => {
                        paging::register(e, &request, reply, &mut pages);
                    }
                    Job::Command(request, reply) => {
                        handle(e, request, reply, &mut pending, &mut readers);
                    }
                }
            }
            Some(ev) = e.events.recv() => {
                e.on_relay_observing_direct_text(ev, &mut |key, text| {
                    answer = approval::matching_reply(&approvals, key, text);
                    ack = ack.take().or_else(|| paging::matching_ack(&pages, key, text));
                });
            }
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
        approval::progress(e, &mut approvals, answer);
        paging::progress(e, &mut pages, ack);
        e.tick();
        watch::progress(e, &mut watches);
        e.happened.clear();
        progress(e, &mut pending, &mut readers);
        groups::progress(e, &mut group_readers);
    }
    drop(approvals);
    drop(pages);
    drop(group_readers);
    // No new commands; what they sent must still reach the relay, as at the
    // end of a one-shot command. systemd waits 90 s before it kills.
    let _ = std::fs::remove_file(&path);
    let until = Instant::now() + STOP_WAIT;
    // The caller says so if this ran out.
    super::pump(e, until, Engine::all_answered).await;
    e.persist();
    Ok(())
}

/// The longest a stop waits for the relay to take what was sent.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// One client: its request lines in, the answers out, until it closes.
async fn connection(stream: UnixStream, jobs: mpsc::Sender<Job>) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let mut line = String::new();
        let n = (&mut read).take(MAX_LINE + 1).read_line(&mut line).await;
        match n {
            Ok(0) | Err(_) => return,
            Ok(_) if line.len() as u64 > MAX_LINE => {
                let _ = write
                    .write_all(format!("{}\n", err(Fail(2, "request too long".into()))).as_bytes())
                    .await;
                return;
            }
            Ok(_) => {}
        }
        let request = serde_json::from_str(&line).unwrap_or(Value::Null);
        if request["cmd"] == "group_recv" {
            let (records, mut answers) = mpsc::channel(groups::BUFFER);
            let (done, result) = oneshot::channel();
            if jobs
                .send(Job::GroupReader(request, records, done))
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                _ = read.fill_buf() => {}
                _ = async {
                    while let Some(record) = answers.recv().await {
                        write.write_all(format!("{record}\n").as_bytes()).await?;
                    }
                    if let Ok(result) = result.await {
                        write.write_all(format!("{result}\n").as_bytes()).await?;
                    }
                    std::io::Result::Ok(())
                } => {}
            }
            return;
        }
        let (reply, mut answers) = mpsc::unbounded_channel();
        let dedicated =
            watch::is_request(&request) || request["cmd"] == "ask" || request["cmd"] == "page";
        if jobs.send(Job::Command(request, reply)).await.is_err() {
            return;
        }
        // Until the last answer, or the client goes away. A client that
        // writes again meanwhile is left alone: its bytes wait for us.
        let mut watch = true;
        loop {
            tokio::select! {
                answer = answers.recv() => {
                    let Some(answer) = answer else { return };
                    let last = serde_json::from_str::<Value>(&answer)
                        .is_ok_and(|v| v.get("ok").is_some());
                    if write.write_all(format!("{answer}\n").as_bytes()).await.is_err() {
                        return;
                    }
                    if last {
                        break;
                    }
                }
                more = read.fill_buf(), if watch => match more {
                    Ok([]) | Err(_) => return,
                    Ok(_) if dedicated => return,
                    Ok(_) => watch = false,
                },
            }
        }
    }
}

fn handle(
    e: &mut Engine,
    request: Value,
    reply: mpsc::UnboundedSender<String>,
    pending: &mut Vec<Pending>,
    readers: &mut Vec<Reader>,
) {
    let s = |k: &str| request[k].as_str().map(str::to_string);
    let timeout = Duration::from_secs(request["timeout"].as_u64().unwrap_or(60));
    let find = |e: &Engine, q: Option<String>| -> Result<usize, Fail> {
        let q = q.ok_or_else(|| Fail(2, "no contact given".into()))?;
        e.find(&q).map_err(|why| Fail(3, why))
    };
    let answer = match request["cmd"].as_str().unwrap_or_default() {
        "groups" | "group_send" | "group_check" => {
            let result = groups::command(e, request, &reply, pending);
            if let Some(answer) = result {
                let _ = reply.send(answer.unwrap_or_else(err));
            }
            return;
        }
        // ADR-035: `run` checks its recipient before its command starts.
        "check" => find(e, s("to"))
            .and_then(|i| can_write(e, i))
            .map(|_| ok(json!({}))),
        "contacts" => {
            // ADR-039: `hook` routes only to verified contacts.
            let verified = e
                .data
                .contacts
                .iter()
                .filter(|c| !c.own)
                .map(|c| c.verified);
            let rows: Vec<Value> = contact_rows(e)
                .into_iter()
                .zip(verified)
                .map(|([name, fingerprint, state], verified)| {
                    json!({ "name": name, "fingerprint": fingerprint, "state": state, "verified": verified })
                })
                .collect();
            Ok(ok(json!({ "contacts": rows })))
        }
        "invite" => match e.data.invite_queue.is_some() && e.data.fallback_published {
            true => e
                .invite()
                .map(|i| ok(json!({ "invite": i })))
                .map_err(|w| Fail(4, w)),
            false => Err(Fail(
                4,
                "the invite queue is not ready yet: try again shortly".into(),
            )),
        },
        "add" => match e.add(&s("invite").unwrap_or_default(), false) {
            Ok(i) => {
                let until = Instant::now() + timeout;
                pending.push(Pending {
                    what: Waiting::Added { i },
                    until,
                    reply,
                });
                return;
            }
            Err(why) => Err(Fail(1, why)),
        },
        cmd @ ("send" | "send_file") => {
            let started = find(e, s("to"))
                .and_then(|i| can_write(e, i))
                .and_then(|i| {
                    let sent = match cmd {
                        "send" => {
                            let text = Zeroizing::new(s("text").unwrap_or_default());
                            if text.trim().is_empty() {
                                return Err(Fail(2, "nothing to send".into()));
                            }
                            match request["expires"].as_u64() {
                                None => e.send(i, &text),
                                Some(s) => e.send_expiring(i, &text, super::expires(s)?),
                            }
                        }
                        _ => {
                            let data = s("data").unwrap_or_default();
                            let bytes = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                data,
                            )
                            .map_err(|_| Fail(2, "the file data is not base64".into()))?;
                            e.send_file(i, &s("name").unwrap_or_default(), bytes)
                        }
                    };
                    sent.map_err(|why| Fail(5, why))?;
                    Ok(i)
                });
            match started {
                Ok(i) => {
                    let m = last_mark(e, i);
                    let until = Instant::now() + timeout;
                    pending.push(Pending {
                        what: Waiting::Sent { i, m },
                        until,
                        reply,
                    });
                    return;
                }
                Err(f) => Err(f),
            }
        }
        "recv" => {
            let from = match s("from") {
                Some(q) => match find(e, Some(q)) {
                    Ok(i) => Some(i),
                    Err(f) => {
                        let _ = reply.send(err(f));
                        return;
                    }
                },
                None => None,
            };
            let until = request["wait"]
                .as_u64()
                .map(|w| Instant::now() + Duration::from_secs(w));
            readers.push(Reader {
                from,
                files: request["files"].as_bool().unwrap_or(false),
                follow: request["follow"].as_bool().unwrap_or(false),
                until,
                got: 0,
                reply,
            });
            return;
        }
        // Audit 2026-11-25, finding 5: a document goes once `recv` wrote it.
        "forget_file" => find(e, s("from")).map(|i| {
            let id = request["id"].as_u64().and_then(|n| u32::try_from(n).ok());
            if let Some(m) = id.and_then(|id| e.file_row(i, id)) {
                e.discard_file(i, m);
            }
            ok(json!({}))
        }),
        _ => Err(Fail(2, format!("unknown request\n{USAGE}"))),
    };
    let _ = reply.send(answer.unwrap_or_else(err));
}

/// Answers what has come to an end: sends on the relay, new messages.
fn progress(e: &mut Engine, pending: &mut Vec<Pending>, readers: &mut Vec<Reader>) {
    let now = Instant::now();
    pending.retain(|p| {
        let done = match p.what {
            Waiting::GroupSent { group, id } => group::send_result(e, &group, id, p.until)
                .map(|result| result.map(|()| ok(json!({})))),
            Waiting::Sent { i, m } => match status_of(e, i, m) {
                Some(Status::Sent) => Some(Ok(ok(json!({})))),
                Some(Status::Failed(why)) => Some(Err(Fail(5, why))),
                // Audit 2026-11-25, finding 6: gone (it expired) is not sent.
                None => Some(Err(Fail(
                    4,
                    "it disappeared before the relay took it".into(),
                ))),
                _ => None,
            },
            Waiting::Added { i } => {
                let c = &e.data.contacts[i];
                (!c.request_pending && !c.offer_pending && c.mine.is_some())
                    .then(|| Ok(ok(json!({ "name": e.name(c) }))))
            }
        };
        let done = done.or_else(|| {
            (now >= p.until).then(|| {
                Err(Fail(
                    4,
                    "the relay did not take it in time; it goes later".into(),
                ))
            })
        });
        match done {
            Some(answer) => {
                let _ = p.reply.send(answer.unwrap_or_else(err));
                false
            }
            None => !p.reply.is_closed(),
        }
    });
    readers.retain(|r| !r.reply.is_closed());
    if readers.is_empty() {
        return;
    }
    // Everyone gets each new message: take once for all, by contact.
    let mut fresh = Vec::new();
    if readers.iter().any(|r| r.from.is_none()) {
        fresh = e.take_new(None);
    } else {
        let mut seen = Vec::new();
        for r in readers.iter() {
            if !seen.contains(&r.from) {
                seen.push(r.from);
                fresh.extend(e.take_new(r.from));
            }
        }
    }
    for (i, m, what) in fresh {
        let wanted = |r: &Reader| r.from.is_none_or(|f| f == i);
        let files = readers.iter().any(|r| wanted(r) && r.files);
        let line = record(e, i, &m, what, files);
        let mut bare = line.clone();
        if let Some(f) = bare.get_mut("_file") {
            f["data"] = Value::Null;
        }
        for r in readers.iter_mut().filter(|r| wanted(r)) {
            let v = if r.files { &line } else { &bare };
            let _ = r.reply.send(v.to_string());
            r.got += 1;
        }
    }
    readers.retain(|r| {
        let waiting = r.until.is_some_and(|u| r.got == 0 && now < u);
        if r.follow || waiting {
            return true;
        }
        let _ = r.reply.send(ok(json!({})));
        false
    });
}

/// The command side: through a running daemon, if one answers for this
/// state folder. `None`: there is none, so the command runs by itself.
pub(super) async fn ask(a: &Args, home: &Path) -> Option<Result<(), Fail>> {
    let stream = UnixStream::connect(home.join(SOCKET)).await.ok()?;
    Some(talk(a, stream, home).await)
}

/// Tells the daemon a document is on disk, so it may let it go.
async fn forget(home: &Path, from: &str, id: u32) -> Result<(), Fail> {
    let gone = |why: std::io::Error| Fail(1, format!("saved, but the daemon keeps it: {why}"));
    let stream = UnixStream::connect(home.join(SOCKET)).await.map_err(gone)?;
    let (read, mut write) = stream.into_split();
    let request = json!({ "cmd": "forget_file", "from": from, "id": id });
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(gone)?;
    let mut line = String::new();
    BufReader::new(read)
        .take(1024)
        .read_line(&mut line)
        .await
        .map_err(gone)?;
    Ok(())
}

async fn talk(a: &Args, stream: UnixStream, home: &Path) -> Result<(), Fail> {
    let request = request(a)?;
    let approval = a.command == "ask" || a.command == "page";
    if (approval || a.group.is_some() || a.command == "groups")
        && !stream
            .peer_cred()
            .is_ok_and(|cred| cred.uid() == rustix::process::geteuid().as_raw())
    {
        return fail(
            if approval { 6 } else { 1 },
            "the daemon socket belongs to another user",
        );
    }
    let (read, mut write) = stream.into_split();
    // Closed before any answer: the daemon turned us away (ADR-029).
    const REFUSED: &str = "the daemon for this identity closed the connection before \
        answering: it lets in only its own user and at most 16 commands at once, or it stopped";
    let closed = || Fail(if approval { 6 } else { 1 }, REFUSED.into());
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|_| closed())?;
    if approval {
        let mut line = String::new();
        BufReader::new(read)
            .take(1024)
            .read_line(&mut line)
            .await
            .map_err(|_| closed())?;
        if !line.ends_with('\n') {
            return Err(closed());
        }
        if a.command == "page" {
            return paging::result(&line);
        }
        return approval::result(&line);
    }
    let mut lines = BufReader::new(read).lines();
    let mut answered = false;
    loop {
        let line = lines.next_line().await;
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) if !answered => return fail(1, REFUSED),
            Ok(None) => break,
            Err(why) => return fail(1, format!("daemon: {why}")),
        };
        answered = true;
        let v: Value =
            serde_json::from_str(&line).map_err(|_| Fail(1, "daemon: bad answer".into()))?;
        let Some(done) = v["ok"].as_bool() else {
            if a.command == "watch" && v["ready"] == true {
                println!("watch ready");
            } else if a.group.is_some() || a.command == "groups" {
                group::print(a, &v)?;
            } else if let Some((from, id)) = print_record(a, v)? {
                forget(home, &from, id).await?;
            }
            continue;
        };
        if !done {
            let code = v["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(1);
            return fail(code, v["error"].as_str().unwrap_or("daemon error"));
        }
        match a.command.as_str() {
            "contacts" => {
                for c in v["contacts"].as_array().into_iter().flatten() {
                    let f = |k: &str| c[k].as_str().unwrap_or_default().to_string();
                    println!("{}\t{}\t{}", f("name"), f("fingerprint"), f("state"));
                }
            }
            "groups" => {
                for row in v["groups"].as_array().into_iter().flatten() {
                    group::print(a, row)?;
                }
            }
            "invite" => println!("{}", v["invite"].as_str().unwrap_or_default()),
            "add" => println!(
                "request sent to {}; they accept it on their device",
                v["name"].as_str().unwrap_or_default()
            ),
            _ => {}
        }
        return Ok(());
    }
    fail(1, "the daemon stopped")
}

/// The request line for the command in `a`.
fn request(a: &Args) -> Result<Value, Fail> {
    if a.group.is_some() || a.command == "groups" {
        return serde_json::to_value(group::request(a)?)
            .map_err(|why| Fail(1, format!("cannot encode group command: {why}")));
    }
    let timeout = a.timeout.unwrap_or(60);
    Ok(match (a.command.as_str(), a.positional.as_slice()) {
        ("contacts", _) => json!({ "cmd": "contacts" }),
        // ADR-040: the chain, then the same limits the daemon checks.
        ("page", [text]) => json!({
            "cmd": "page",
            "to": a.to.iter().chain(&a.then).collect::<Vec<_>>(),
            "text": text,
            "key": a.key,
            "every": a.every,
            "escalate": a.escalate,
            "give_up": a.give_up,
            "wait": true,
        }),
        ("resolve", []) => json!({ "cmd": "resolve", "key": a.key }),
        ("invite", _) => json!({ "cmd": "invite" }),
        ("add", [invite]) => json!({ "cmd": "add", "invite": invite, "timeout": timeout }),
        ("send", [to, text]) => {
            json!({ "cmd": "send", "to": to, "text": text, "timeout": timeout, "expires": a.expires })
        }
        ("send", [to]) => {
            let mut text = Zeroizing::new(String::new());
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .map_err(|e| Fail(1, format!("cannot read stdin: {e}")))?;
            let text = text.strip_suffix('\n').unwrap_or(&text);
            json!({ "cmd": "send", "to": to, "text": text, "timeout": timeout, "expires": a.expires })
        }
        ("send-file", [to, file]) => {
            // The command reads the file as its own user; the daemon opens no paths.
            let (name, bytes) =
                data::read_document(&data::expand(file)).map_err(|why| Fail(1, why))?;
            let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
            json!({ "cmd": "send_file", "to": to, "name": name, "data": data, "timeout": timeout })
        }
        ("recv", _) => {
            // The daemon hands documents over for good: the folder must be there.
            if let Some(dir) = &a.save_dir
                && !dir.is_dir()
            {
                return fail(1, format!("{} is not a folder", dir.display()));
            }
            json!({
                "cmd": "recv", "from": a.from, "wait": a.wait, "follow": a.follow,
                "files": a.save_dir.is_some(),
            })
        }
        ("watch", [role, peer]) if role == "send" => {
            json!({ "cmd": "watch_send", "peer": peer, "seconds": a.every })
        }
        ("watch", [role, peer]) if role == "monitor" => {
            json!({ "cmd": "watch_monitor", "peer": peer, "owner": a.notify, "seconds": a.after })
        }
        ("run", _) => json!({ "cmd": "check", "to": a.to }),
        ("ask", [to, text]) => json!({
            "cmd": "ask", "to": to, "text": text, "timeout": a.timeout.unwrap_or(600)
        }),
        _ => return fail(2, format!("wrong arguments for {}\n{USAGE}", a.command)),
    })
}
