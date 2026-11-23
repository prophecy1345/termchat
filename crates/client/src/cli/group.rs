use std::io::Write;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use termchat_core::proto::{GroupId, MAX_TEXT_BYTES};
use termchat_engine::data::Group;
use termchat_engine::engine::GroupScriptError;
use termchat_engine::{Engine, clean, engine::clean_text};
use zeroize::Zeroizing;

use super::{Args, Fail, fail, pump, send_input};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Selector {
    Name(String),
    Id(GroupId),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Groups,
    GroupSend {
        group: Selector,
        text: String,
        timeout: u64,
    },
    GroupRecv {
        group: Selector,
        wait: Option<u64>,
        follow: bool,
    },
    /// ADR-035: `run` checks the group before its command starts.
    GroupCheck {
        group: Selector,
    },
}

pub(super) fn deadline(seconds: u64) -> Result<Instant, Fail> {
    Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or_else(|| Fail(2, "duration is too large".into()))
}

pub(super) fn parse_id(query: &str) -> Result<GroupId, Fail> {
    let mut id = [0; 16];
    if query.len() != 32 || !query.bytes().all(|b| b.is_ascii_hexdigit()) {
        return fail(2, "a group ID is 32 hexadecimal digits");
    }
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&query[2 * i..2 * i + 2], 16)
            .map_err(|_| Fail(2, "invalid group ID".into()))?;
    }
    Ok(id)
}

fn group_id(id: GroupId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

impl Selector {
    fn validate(&self) -> Result<(), Fail> {
        if matches!(self, Self::Name(name) if name.is_empty()) {
            return fail(2, "a group name cannot be empty");
        }
        Ok(())
    }

    pub(super) fn locate(&self, e: &Engine) -> Result<usize, Fail> {
        self.validate()?;
        let mut found = None;
        let mut inaccessible = false;
        for (i, group) in e.data.groups.iter().enumerate() {
            let matches = match self {
                Self::Name(name) => clean(group.name()) == *name,
                Self::Id(id) => group.id == *id,
            };
            if !matches {
                continue;
            }
            if !e.group_access(i) {
                inaccessible = true;
                continue;
            }
            if found.replace(i).is_some() {
                return fail(3, "more than one group has this name; use --group-id");
            }
        }
        match found {
            Some(i) => Ok(i),
            None if inaccessible => fail(5, "this device is not in the group"),
            None => fail(3, "no such group"),
        }
    }
}

pub(super) fn validate_args(a: &Args) -> Result<(), Fail> {
    if let Some(group) = &a.group {
        group.validate()?;
    }
    if a.from.is_some() || a.save_dir.is_some() {
        return fail(2, "group commands do not accept --from or --save-dir");
    }
    if a.nickname.is_some()
        || a.relay.is_some()
        || a.onion.is_some()
        || a.tor
        || !match a.command.as_str() {
            "groups" => a.group.is_none() && a.wait.is_none() && !a.follow,
            "send" => a.wait.is_none() && !a.follow && !a.json,
            "recv" => a.wait.is_none() || !a.follow,
            "run" => a.wait.is_none() && !a.follow && !a.json,
            "systemd-unit" => a.wait.is_none() && !a.follow && !a.json,
            _ => false,
        }
    {
        return fail(2, "incompatible group command options");
    }
    deadline(a.timeout.unwrap_or(60))?;
    receive_deadline(a.wait, a.follow)?;
    Ok(())
}

pub(super) fn request(a: &Args) -> Result<Request, Fail> {
    Ok(match (&a.group, a.command.as_str()) {
        (None, "groups") => Request::Groups,
        (Some(group), "send") => {
            let text = send_input(a, a.positional.first())?;
            let text = text.strip_suffix('\n').unwrap_or(&text);
            validate_text(text)?;
            Request::GroupSend {
                group: group.clone(),
                text: text.into(),
                timeout: a.timeout.unwrap_or(60),
            }
        }
        (Some(group), "run") => Request::GroupCheck {
            group: group.clone(),
        },
        (Some(group), "recv") => Request::GroupRecv {
            group: group.clone(),
            wait: a.wait,
            follow: a.follow,
        },
        _ => return fail(2, "expected a group command"),
    })
}

pub(super) fn validate_text(text: &str) -> Result<(), Fail> {
    if text.trim().is_empty() {
        return fail(2, "nothing to send");
    }
    if text.len() > MAX_TEXT_BYTES {
        return fail(2, "message too long");
    }
    Ok(())
}

pub(super) fn receive_deadline(wait: Option<u64>, follow: bool) -> Result<Option<Instant>, Fail> {
    if follow && wait.is_some() {
        return fail(2, "use one of --wait and --follow");
    }
    wait.map(deadline).transpose()
}

pub(super) fn engine_error(error: GroupScriptError) -> Fail {
    match error {
        GroupScriptError::State(why) => Fail(1, why),
        GroupScriptError::Refused(why) => Fail(5, why),
    }
}

pub(super) fn send_result(
    e: &Engine,
    group: &GroupId,
    id: u64,
    until: Instant,
) -> Option<Result<(), Fail>> {
    match e.group_text_accepted(group, id) {
        Ok(Some(at)) if at <= until => Some(Ok(())),
        Ok(_) if Instant::now() >= until => Some(fail(
            4,
            "the relay did not take the group text in time; it may go later",
        )),
        Ok(_) => None,
        Err(error) => Some(Err(engine_error(error))),
    }
}

fn row(group: &Group) -> Value {
    json!({ "id": group_id(group.id), "name": clean(group.name()) })
}

pub(super) fn rows(e: &Engine) -> Vec<Value> {
    e.data
        .groups
        .iter()
        .enumerate()
        .filter(|(g, _)| e.group_access(*g))
        .map(|(_, group)| row(group))
        .collect()
}

pub(super) fn take(e: &mut Engine, id: &GroupId, limit: usize) -> Result<(Vec<Value>, bool), Fail> {
    let g = e
        .group_index(id)
        .ok_or_else(|| Fail(5, "this device is not in the group".into()))?;
    let (texts, more) = e.take_group_texts(g, limit).map_err(engine_error)?;
    let group = &e.data.groups[g];
    let records = texts
        .iter()
        .filter_map(|m| {
            let (id, from) = (m.id?, m.author?);
            let mut record = json!({
                "group_id": group_id(group.id), "group": clean(group.name()),
                "from": clean(group.name_of(&from).unwrap_or("someone")),
                "fingerprint": termchat_core::fingerprint(&from),
                "at": m.at, "id": id.to_string(), "text": clean_text(&m.text),
            });
            if let Some(reply) = m.reply_to {
                record["reply_to"] = reply.id.to_string().into();
            }
            Some(record)
        })
        .collect();
    Ok((records, more))
}

pub(super) fn print(a: &Args, v: &Value) -> Result<(), Fail> {
    let mut out = std::io::stdout().lock();
    let s = |key| v[key].as_str().unwrap_or_default();
    let written = if a.json {
        writeln!(out, "{v}")
    } else if a.command == "groups" {
        writeln!(out, "{}\t{}", s("id"), s("name"))
    } else {
        let reply = v["reply_to"]
            .as_str()
            .map_or_else(String::new, |id| format!(" ↪ {id}"));
        writeln!(
            out,
            "{} [{}] {} [{}] {} {}{}: {}",
            s("group"),
            s("group_id"),
            s("from"),
            s("fingerprint"),
            v["at"],
            s("id"),
            reply,
            s("text").replace('\n', "\n  ")
        )
    };
    written
        .and_then(|()| out.flush())
        .map_err(|e| Fail(1, format!("cannot write: {e}")))
}

pub(super) async fn run(e: &mut Engine, a: &Args, until: Instant) -> Result<(), Fail> {
    match request(a)? {
        Request::Groups => {
            for v in rows(e) {
                print(a, &v)?;
            }
        }
        Request::GroupSend {
            group,
            text,
            timeout,
        } => {
            let text = Zeroizing::new(text);
            let until = deadline(timeout)?;
            let g = group.locate(e)?;
            let group = e.data.groups[g].id;
            let id = e.send_group_text(g, &text).map_err(engine_error)?;
            drop(text);
            pump(e, until, |e| send_result(e, &group, id, until).is_some()).await;
            return send_result(e, &group, id, until)
                .unwrap_or_else(|| fail(1, "missing group delivery result"));
        }
        Request::GroupCheck { group } => {
            group.locate(e)?;
        }
        Request::GroupRecv {
            group,
            wait,
            follow,
        } => {
            let group = e.data.groups[group.locate(e)?].id;
            let until = if !follow && wait.is_none() {
                pump(e, until, Engine::all_connected).await;
                Some(until.min(Instant::now() + Duration::from_secs(3)))
            } else {
                receive_deadline(wait, follow)?
            };
            loop {
                let (records, _) = take(e, &group, usize::MAX)?;
                let got = !records.is_empty();
                for v in records {
                    print(a, &v)?;
                }
                if !follow && (got || until.is_some_and(|end| Instant::now() >= end)) {
                    break;
                }
                let next = Instant::now() + Duration::from_millis(200);
                pump(e, until.map_or(next, |end| next.min(end)), |_| false).await;
            }
        }
    }
    Ok(())
}
