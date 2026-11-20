use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{Value, json};
use termchat_core::proto::GroupId;
use termchat_engine::Engine;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::{Fail, Pending, Waiting, err, fail, group, ok};
use group::Request;

pub(super) const BUFFER: usize = 64;
const BATCH: usize = 16;

pub(super) struct Reader {
    group: GroupId,
    until: Option<Instant>,
    follow: bool,
    got: bool,
    records: mpsc::Sender<String>,
    done: Option<oneshot::Sender<String>>,
}

fn decode(request: Value) -> Result<Request, Fail> {
    serde_json::from_value(request).map_err(|_| Fail(2, "invalid group request".into()))
}

pub(super) fn command(
    e: &mut Engine,
    request: Value,
    reply: &mpsc::UnboundedSender<String>,
    pending: &mut Vec<Pending>,
) -> Option<Result<String, Fail>> {
    let start = || -> Result<_, Fail> {
        match decode(request)? {
            Request::Groups => Ok(Some(ok(json!({ "groups": group::rows(e) })))),
            Request::GroupSend {
                group,
                text,
                timeout,
            } => {
                let text = Zeroizing::new(text);
                let until = group::deadline(timeout)?;
                group::validate_text(&text)?;
                let g = group.locate(e)?;
                let group = e.data.groups[g].id;
                let id = e.send_group_text(g, &text).map_err(group::engine_error)?;
                pending.push(Pending {
                    what: Waiting::GroupSent { group, id },
                    until,
                    reply: reply.clone(),
                });
                Ok(None)
            }
            _ => fail(2, "expected a group list or send request"),
        }
    };
    start().transpose()
}

pub(super) fn register(
    e: &Engine,
    request: Value,
    records: mpsc::Sender<String>,
    done: oneshot::Sender<String>,
    readers: &mut Vec<Reader>,
) {
    let start = || -> Result<_, Fail> {
        let Request::GroupRecv {
            group,
            wait,
            follow,
        } = decode(request)?
        else {
            return fail(2, "expected a group read request");
        };
        let until = group::receive_deadline(wait, follow)?;
        let group = e.data.groups[group.locate(e)?].id;
        Ok((group, until, follow))
    };
    match start() {
        Ok((group, until, follow)) => readers.push(Reader {
            group,
            until,
            follow,
            got: false,
            records,
            done: Some(done),
        }),
        Err(error) => {
            let _ = done.send(err(error));
        }
    }
}

pub(super) fn progress(e: &mut Engine, readers: &mut Vec<Reader>) {
    readers.retain(|r| !r.records.is_closed());
    let mut batches = BTreeMap::new();
    for r in readers.iter() {
        batches.entry(r.group).or_insert_with(|| {
            group::take(e, &r.group, BATCH)
                .map(|(batch, more)| {
                    (
                        batch.into_iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                        more,
                    )
                })
                .map_err(err)
        });
    }
    let now = Instant::now();
    readers.retain_mut(|r| publish(r, &batches[&r.group], now));
}

fn publish(r: &mut Reader, batch: &Result<(Vec<String>, bool), String>, now: Instant) -> bool {
    let result = match batch {
        Err(error) => error.clone(),
        Ok((batch, more)) => {
            r.got |= !batch.is_empty();
            let sent = batch
                .iter()
                .try_for_each(|line| r.records.try_send(line.clone()));
            match sent {
                Err(mpsc::error::TrySendError::Full(_)) => {
                    err(Fail(1, "group reader is too slow".into()))
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return false,
                Ok(()) if r.follow || *more || (!r.got && r.until.is_some_and(|end| now < end)) => {
                    return true;
                }
                Ok(()) => ok(json!({})),
            }
        }
    };
    // The final status has a separate slot even if all 64 records are buffered.
    if let Some(done) = r.done.take() {
        let _ = done.send(result);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn overflow_finishes_only_the_slow_reader() {
        let subscriber = || {
            let (records, receive) = mpsc::channel(BUFFER);
            let (done, result) = oneshot::channel();
            (
                Reader {
                    group: [1; 16],
                    until: None,
                    follow: true,
                    got: false,
                    records,
                    done: Some(done),
                },
                receive,
                result,
            )
        };
        let (mut slow, mut slow_rx, mut slow_done) = subscriber();
        let (mut fast, mut fast_rx, mut fast_done) = subscriber();
        for _ in 0..BUFFER {
            slow.records.try_send("old".into()).unwrap();
        }
        let batch = Ok((vec!["new".into()], false));
        let now = Instant::now();
        assert!(!publish(&mut slow, &batch, now));
        assert!(slow_done.try_recv().unwrap().contains("too slow"));
        assert_eq!(slow_rx.len(), BUFFER);
        assert_eq!(slow_rx.try_recv().unwrap(), "old");
        assert!(publish(&mut fast, &batch, now));
        assert_eq!(fast_rx.try_recv().unwrap(), "new");
        assert!(matches!(
            fast_done.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }
}
