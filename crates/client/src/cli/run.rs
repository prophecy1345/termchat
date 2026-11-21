//! ADR-035: `termchat run` runs a command and reports how it ended.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use termchat_engine::{clean, engine::clean_text};

/// Each kept line is cut here, and the whole tail at `MAX_TAIL`.
const MAX_LINE: usize = 500;
const MAX_TAIL: usize = 12_000;
pub(super) const DEFAULT_LINES: usize = 10;
pub(super) const MAX_LINES: usize = 50;
pub(super) const MAX_NAME: usize = 100;

/// How the command ended, and its exit status for `run`.
pub(super) struct Outcome {
    pub code: i32,
    pub text: String,
}

type Tail = Arc<Mutex<VecDeque<Vec<u8>>>>;

/// Runs `program args`, passing its output through and keeping the last
/// `lines` lines; the report text and the status to exit with.
pub(super) fn run(label: &str, program: &str, args: &[String], lines: usize) -> Outcome {
    let started = Instant::now();
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(why) => {
            return Outcome {
                code: 127,
                text: format!("{} could not start: {why} on {}", clean(label), host()),
            };
        }
    };
    let tail: Tail = Arc::default();
    let readers = [
        child
            .stdout
            .take()
            .map(|out| copy(out, std::io::stdout(), &tail, lines)),
        child
            .stderr
            .take()
            .map(|err| copy(err, std::io::stderr(), &tail, lines)),
    ];
    let status = child.wait();
    // A background process the command left behind can hold the pipes open
    // for good: give the readers a moment, then report what they have.
    let settle = Instant::now() + Duration::from_secs(1);
    while readers.iter().flatten().any(|r| !r.is_finished()) && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(20));
    }
    let took = duration(started.elapsed());
    let (code, outcome) = match status {
        Ok(status) => ended(status),
        Err(why) => (1, format!("could not be waited for: {why}")),
    };
    let mut text = format!("{} {outcome} after {took} on {}", clean(label), host());
    let kept = tail.lock().unwrap_or_else(PoisonError::into_inner);
    let mut room = MAX_TAIL;
    let mut shown = Vec::new();
    for line in kept.iter().rev() {
        let line = clean_text(&String::from_utf8_lossy(line));
        if line.len() + 1 > room {
            break;
        }
        room -= line.len() + 1;
        shown.push(line);
    }
    for line in shown.into_iter().rev() {
        text.push('\n');
        text.push_str(&line);
    }
    Outcome { code, text }
}

/// The program's file name, never its arguments (ADR-035).
pub(super) fn label(program: &str) -> String {
    std::path::Path::new(program)
        .file_name()
        .map_or_else(|| program.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Copies `from` to `to` as it comes and keeps the last `lines` lines.
fn copy(
    mut from: impl Read + Send + 'static,
    mut to: impl Write + Send + 'static,
    tail: &Tail,
    lines: usize,
) -> std::thread::JoinHandle<()> {
    let tail = Arc::clone(tail);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut line = Vec::new();
        let keep = |line: &mut Vec<u8>| {
            if lines > 0 {
                let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if tail.len() == lines {
                    tail.pop_front();
                }
                tail.push_back(std::mem::take(line));
            }
            line.clear();
        };
        while let Ok(n @ 1..) = from.read(&mut buf) {
            // The caller's log gets the output even if nobody reads ours.
            let _ = to.write_all(&buf[..n]).and_then(|()| to.flush());
            for part in buf[..n].split_inclusive(|&b| b == b'\n') {
                let (text, end) = match part.strip_suffix(b"\n") {
                    Some(text) => (text, true),
                    None => (part, false),
                };
                let room = MAX_LINE.saturating_sub(line.len());
                line.extend_from_slice(&text[..text.len().min(room)]);
                if end {
                    keep(&mut line);
                }
            }
        }
        if !line.is_empty() {
            keep(&mut line);
        }
    })
}

fn ended(status: ExitStatus) -> (i32, String) {
    if let Some(code) = status.code() {
        return match code {
            0 => (0, "finished".into()),
            n => (n, format!("failed with exit {n}")),
        };
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return (128 + signal, format!("was killed by signal {signal}"));
    }
    (1, "ended without a status".into())
}

fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

fn host() -> String {
    #[cfg(unix)]
    let name = rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned();
    #[cfg(not(unix))]
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    match clean(&name) {
        n if n.is_empty() => "this host".into(),
        n => n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(duration(Duration::from_secs(3)), "3s");
        assert_eq!(duration(Duration::from_secs(2530)), "42m 10s");
        assert_eq!(duration(Duration::from_secs(3725)), "1h 2m 5s");
    }

    #[test]
    fn labels_drop_paths_and_arguments() {
        assert_eq!(label("/usr/local/bin/backup.sh"), "backup.sh");
        assert_eq!(label("make"), "make");
    }

    #[cfg(unix)]
    #[test]
    fn reports_keep_a_bounded_tail() {
        let script = "i=0; while [ $i -lt 200 ]; do echo line $i; i=$((i+1)); done; \
                      head -c 3000000 /dev/zero | tr '\\0' x; echo; printf '\\033[31mred\\n'; exit 3";
        let args = ["-c".to_string(), script.to_string()];
        let out = run("job", "sh", &args, 3);
        assert_eq!(out.code, 3);
        let lines: Vec<&str> = out.text.lines().collect();
        assert!(
            lines[0].starts_with("job failed with exit 3 after "),
            "{}",
            lines[0]
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1], "line 199");
        assert_eq!(lines[2].len(), MAX_LINE, "a long line is cut");
        assert!(
            !lines[3].contains('\u{1b}'),
            "control characters are removed"
        );

        let out = run(
            "err",
            "sh",
            &["-c".into(), "echo oops >&2; exit 1".into()],
            10,
        );
        assert_eq!(out.text.lines().nth(1), Some("oops"), "stderr is kept too");

        let out = run("job", "sh", &["-c".into(), "kill -9 $$".into()], 10);
        assert_eq!(out.code, 137);
        assert!(out.text.starts_with("job was killed by signal 9 after "));

        let out = run("nope", "/nonexistent/program", &[], 10);
        assert_eq!(out.code, 127);
        assert!(out.text.starts_with("nope could not start: "));

        let started = Instant::now();
        let out = run(
            "bg",
            "sh",
            &["-c".into(), "sleep 30 & echo left".into()],
            10,
        );
        assert_eq!(out.code, 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a background job must not hold run"
        );
        assert_eq!(out.text.lines().nth(1), Some("left"));

        let out = run("quiet", "sh", &["-c".into(), "echo hidden".into()], 0);
        assert_eq!(out.code, 0);
        assert_eq!(out.text.lines().count(), 1);
        assert!(out.text.starts_with("quiet finished after "));
    }
}
