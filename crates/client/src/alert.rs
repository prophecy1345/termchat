//! ADR-039: a monitoring webhook's body as one plain-text message.
//! No markup (rule 9); only the labels an operator names reach the chat.

use serde_json::Value;
use termchat_engine::engine::clean_text;

/// At most this many alerts in one message, then "and N more".
pub const MAX_ALERTS: usize = 10;
/// At most this many characters in one message, cut at a line end.
pub const MAX_CHARS: usize = 4000;

/// What a route expects in the request body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Alertmanager if `alerts` is an array, Kuma if `msg` is a string, else text.
    Auto,
    /// Alertmanager's webhook JSON, also sent by Grafana's unified alerting.
    Alertmanager,
    /// Uptime Kuma's JSON.
    Kuma,
    /// The body as text (Gatus and anything that can template a body).
    Text,
}

impl Format {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "auto" => Self::Auto,
            "alertmanager" => Self::Alertmanager,
            "kuma" => Self::Kuma,
            "text" => Self::Text,
            _ => return None,
        })
    }
}

/// The message for `body`, or `None` if the body is not in `format`.
/// `labels`: label names to show besides alertname and instance.
pub fn render(format: Format, body: &[u8], labels: &[String]) -> Option<String> {
    let text = match format {
        Format::Text => plain(body)?,
        Format::Alertmanager => alertmanager(&serde_json::from_slice(body).ok()?, labels)?,
        Format::Kuma => kuma(&serde_json::from_slice(body).ok()?)?,
        Format::Auto => match serde_json::from_slice::<Value>(body) {
            Ok(v) if v["alerts"].is_array() => alertmanager(&v, labels)?,
            Ok(v) if v["msg"].is_string() => kuma(&v)?,
            _ => plain(body)?,
        },
    };
    let text = cut(&clean_text(&text));
    (!text.trim().is_empty()).then_some(text)
}

fn plain(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn kuma(v: &Value) -> Option<String> {
    let msg = v["msg"].as_str()?.trim();
    (!msg.is_empty()).then(|| msg.to_string())
}

/// One line for the group's status, one per alert.
fn alertmanager(v: &Value, labels: &[String]) -> Option<String> {
    let alerts = v["alerts"].as_array()?;
    let status = v["status"].as_str().unwrap_or("firing");
    let mut out = format!("{} ({})", status.to_uppercase(), alerts.len());
    for alert in alerts.iter().take(MAX_ALERTS) {
        out.push('\n');
        out.push_str(&line(alert, status, labels));
    }
    if alerts.len() > MAX_ALERTS {
        out.push_str(&format!("\nand {} more", alerts.len() - MAX_ALERTS));
    }
    Some(out)
}

fn line(alert: &Value, group_status: &str, labels: &[String]) -> String {
    let label = |k: &str| {
        alert["labels"][k]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let mut out = String::from("- ");
    if let Some(s) = alert["status"].as_str().filter(|s| *s != group_status) {
        out.push_str(&format!("[{s}] "));
    }
    out.push_str(label("alertname").unwrap_or("alert"));
    if let Some(instance) = label("instance") {
        out.push_str(&format!(" on {instance}"));
    }
    let note = ["summary", "description"].iter().find_map(|k| {
        alert["annotations"][k]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    });
    if let Some(note) = note {
        out.push_str(&format!(": {note}"));
    }
    let extra: Vec<String> = labels
        .iter()
        .filter_map(|k| label(k).map(|v| format!("{k}={v}")))
        .collect();
    if !extra.is_empty() {
        out.push_str(&format!(" [{}]", extra.join(", ")));
    }
    // One alert, one line: a newline inside a note would read as another alert.
    out.replace('\n', " ")
}

/// One incident in a webhook, for a route that pages (ADR-040).
#[derive(Debug, PartialEq, Eq)]
pub struct Incident {
    /// The same problem keeps the same key, so a repeat merges and a
    /// resolution finds its page.
    pub key: String,
    pub text: String,
    pub firing: bool,
}

/// The incidents in `body`, or `None` if it is not in `format`. Alertmanager
/// gives one per alert (keyed by its fingerprint), Kuma one per monitor
/// (down pages, up resolves), plain text one keyed by `fallback`.
pub fn incidents(
    format: Format,
    body: &[u8],
    labels: &[String],
    fallback: &str,
) -> Option<Vec<Incident>> {
    let json = serde_json::from_slice::<Value>(body).ok();
    let kind = match (format, &json) {
        (Format::Alertmanager, _) | (Format::Auto, Some(Value::Object(_)))
            if json.as_ref().is_some_and(|v| v["alerts"].is_array()) =>
        {
            Format::Alertmanager
        }
        (Format::Kuma, _) | (Format::Auto, Some(_))
            if json.as_ref().is_some_and(|v| v["msg"].is_string()) =>
        {
            Format::Kuma
        }
        (Format::Alertmanager | Format::Kuma, _) => return None,
        _ => Format::Text,
    };
    let list = match (kind, json) {
        (Format::Alertmanager, Some(v)) => {
            let status = v["status"].as_str().unwrap_or("firing");
            v["alerts"]
                .as_array()?
                .iter()
                .take(MAX_ALERTS)
                .map(|alert| {
                    let firing = alert["status"].as_str().unwrap_or(status) != "resolved";
                    let label = |k: &str| alert["labels"][k].as_str().unwrap_or_default();
                    let key = alert["fingerprint"]
                        .as_str()
                        .filter(|f| !f.is_empty())
                        .map_or_else(
                            || format!("{}@{}", label("alertname"), label("instance")),
                            String::from,
                        );
                    // Its own status is in the page's wording; leave it out of the line.
                    let text = line(alert, alert["status"].as_str().unwrap_or(status), labels);
                    Incident {
                        key: format!("am:{}", clean_text(&key)),
                        text: cut(&clean_text(text.trim_start_matches("- "))),
                        firing,
                    }
                })
                .collect()
        }
        (Format::Kuma, Some(v)) => {
            let monitor = v["monitor"]["name"].as_str().unwrap_or(fallback);
            vec![Incident {
                key: format!("kuma:{}", clean_text(monitor)),
                text: cut(&clean_text(v["msg"].as_str()?.trim())),
                // Kuma: 0 down, 1 up, 2 pending, 3 maintenance.
                firing: v["heartbeat"]["status"].as_i64() != Some(1),
            }]
        }
        _ => vec![Incident {
            key: format!("hook:{fallback}"),
            text: cut(&clean_text(&plain(body)?)),
            firing: true,
        }],
    };
    let list: Vec<Incident> = list
        .into_iter()
        .filter(|i| !i.text.trim().is_empty())
        .collect();
    (!list.is_empty()).then_some(list)
}

/// At most `MAX_CHARS`, cut after the last whole line that fits.
fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_CHARS - 2).collect();
    let end = head.rfind('\n').unwrap_or(head.len());
    format!("{}\n…", &head[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const AM: &str = r#"{"status":"firing","alerts":[
        {"status":"firing","labels":{"alertname":"DiskFull","instance":"web-1:9100","job":"node","secret_host":"10.0.0.7"},
         "annotations":{"summary":"Disk 95% full","description":"long"}},
        {"status":"resolved","labels":{"alertname":"HighLoad"},"annotations":{"description":"load\nback to normal"}}]}"#;

    #[test]
    fn alertmanager_keeps_name_instance_note_and_only_named_labels() {
        let text = render(Format::Auto, AM.as_bytes(), &["job".into()]).unwrap();
        assert_eq!(
            text,
            "FIRING (2)\n- DiskFull on web-1:9100: Disk 95% full [job=node]\n\
             - [resolved] HighLoad: load back to normal"
        );
        assert!(!text.contains("10.0.0.7"));
    }

    #[test]
    fn kuma_text_and_auto() {
        let kuma_body = br#"{"msg":"[web] is down","heartbeat":{"status":0}}"#;
        assert_eq!(
            render(Format::Auto, kuma_body, &[]).unwrap(),
            "[web] is down"
        );
        assert_eq!(
            render(Format::Kuma, kuma_body, &[]).unwrap(),
            "[web] is down"
        );
        assert_eq!(
            render(Format::Auto, b"  gatus: api down \n", &[]).unwrap(),
            "gatus: api down"
        );
    }

    #[test]
    fn wrong_or_empty_bodies_are_refused() {
        assert_eq!(render(Format::Alertmanager, b"not json", &[]), None);
        assert_eq!(render(Format::Alertmanager, br#"{"alerts":{}}"#, &[]), None);
        assert_eq!(render(Format::Kuma, br#"{"msg":"  "}"#, &[]), None);
        assert_eq!(render(Format::Text, b"\xff\xfe", &[]), None);
        assert_eq!(render(Format::Auto, b"   ", &[]), None);
    }

    #[test]
    fn many_alerts_and_long_text_are_bounded() {
        let one = r#"{"labels":{"alertname":"A"},"annotations":{"summary":"x"}}"#;
        let many = format!(r#"{{"alerts":[{}]}}"#, vec![one; 25].join(","));
        let text = render(Format::Alertmanager, many.as_bytes(), &[]).unwrap();
        assert_eq!(text.lines().count(), 1 + MAX_ALERTS + 1);
        assert!(text.ends_with("and 15 more"));

        let long = "line of text\n".repeat(1000);
        let text = render(Format::Text, long.as_bytes(), &[]).unwrap();
        assert!(text.chars().count() <= MAX_CHARS);
        assert!(text.ends_with("text\n…"));
    }

    #[test]
    fn incidents_key_by_fingerprint_monitor_or_route() {
        let am = r#"{"status":"firing","alerts":[
            {"status":"firing","fingerprint":"a1","labels":{"alertname":"DiskFull","instance":"web-1"},"annotations":{"summary":"95%"}},
            {"status":"resolved","labels":{"alertname":"Load","instance":"db"}}]}"#;
        let got = incidents(Format::Auto, am.as_bytes(), &[], "oncall").unwrap();
        assert_eq!(
            got[0],
            Incident {
                key: "am:a1".into(),
                text: "DiskFull on web-1: 95%".into(),
                firing: true
            }
        );
        assert_eq!(
            got[1],
            Incident {
                key: "am:Load@db".into(),
                text: "Load on db".into(),
                firing: false
            }
        );

        let down = br#"{"msg":"[api] down","monitor":{"name":"api"},"heartbeat":{"status":0}}"#;
        let up = br#"{"msg":"[api] up","monitor":{"name":"api"},"heartbeat":{"status":1}}"#;
        let d = incidents(Format::Auto, down, &[], "r").unwrap();
        let u = incidents(Format::Kuma, up, &[], "r").unwrap();
        assert_eq!((d[0].key.as_str(), d[0].firing), ("kuma:api", true));
        assert_eq!((u[0].key.as_str(), u[0].firing), ("kuma:api", false));

        let t = incidents(Format::Text, b"gatus: api down", &[], "oncall").unwrap();
        assert_eq!(
            t,
            vec![Incident {
                key: "hook:oncall".into(),
                text: "gatus: api down".into(),
                firing: true
            }]
        );
        assert_eq!(incidents(Format::Alertmanager, b"{}", &[], "r"), None);
        assert_eq!(incidents(Format::Auto, b"  ", &[], "r"), None);
    }

    #[test]
    fn control_characters_and_bidi_overrides_are_removed() {
        let body = "evil\u{202e}txt.exe\u{7}".as_bytes();
        let text = render(Format::Text, body, &[]).unwrap();
        assert!(!text.contains('\u{202e}') && !text.contains('\u{7}'));
    }
}
