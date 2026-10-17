//! UnifiedPush wake-ups (ADR-012): an empty POST to the endpoint a key chose.
//!
//! The relay must not become a proxy into networks it can reach: only
//! `https://`, only public addresses, and it connects to the address it
//! checked, so the name cannot be re-resolved in between. No redirects; the
//! answer is read and dropped.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::http::Uri;
use termchat_proto::MAX_PUSH_URL_BYTES;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Host, port and path of an acceptable endpoint.
pub fn parse(url: &str) -> Option<(String, u16, String)> {
    if url.len() > MAX_PUSH_URL_BYTES {
        return None;
    }
    let uri: Uri = url.parse().ok()?;
    if uri.scheme_str() != Some("https") || uri.authority()?.as_str().contains('@') {
        return None;
    }
    let path = uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    Some((uri.host()?.to_owned(), uri.port_u16().unwrap_or(443), path))
}

/// Only addresses on the public internet.
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || first & 0xfe00 == 0xfc00 // unique local
                    || first & 0xffc0 == 0xfe80 // link-local
                    || (first == 0x2001 && v6.segments()[1] == 0x0db8) // documentation
                    || (first == 0x64 && v6.segments()[1] == 0xff9b)) // NAT64 can reach IPv4 anywhere
            }
        },
    }
}

fn public_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && b & 0xc0 == 64) // shared address space
        || (a == 192 && b == 0 && v4.octets()[2] == 0)
        || (a == 198 && (b == 18 || b == 19))) // benchmarking
}

fn tls() -> TlsConnector {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(
                tokio_rustls::rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap_or_else(|e| panic!("rustls defaults: {e}"))
            .with_root_certificates(roots)
            .with_no_client_auth(),
        )
    });
    TlsConnector::from(Arc::clone(config))
}

/// Sends the wake-up. Errors are only logged: a push is a courtesy.
pub async fn wake(url: String) {
    if let Err(e) = tokio::time::timeout(TIMEOUT, post(&url))
        .await
        .unwrap_or(Err("timeout".into()))
    {
        tracing::debug!("push failed: {e}");
    }
}

async fn post(url: &str) -> Result<(), String> {
    let (host, port, path) = parse(url).ok_or("bad endpoint")?;
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?
        .collect();
    // Every address the name has must be public, then connect to one we checked.
    if addrs.is_empty() || !addrs.iter().all(|a| public(a.ip())) {
        return Err("endpoint is not a public address".into());
    }
    let tcp = TcpStream::connect(addrs[0])
        .await
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let mut stream = tls().connect(name, tcp).await.map_err(|e| e.to_string())?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut answer = [0u8; 512];
    let _ = stream.read(&mut answer).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Network: `TERMCHAT_PUSH_URL=https://ntfy.sh/<topic> cargo test -p termchat-relay -- --ignored`,
    /// then look at the topic.
    #[tokio::test]
    #[ignore]
    async fn wakes_a_real_endpoint() {
        let url = std::env::var("TERMCHAT_PUSH_URL").unwrap_or_default();
        assert!(post(&url).await.is_ok());
    }

    #[test]
    fn only_https_endpoints_on_public_addresses() {
        assert_eq!(
            parse("https://ntfy.sh/upAbc?x=1"),
            Some(("ntfy.sh".into(), 443, "/upAbc?x=1".into()))
        );
        for bad in [
            "http://ntfy.sh/up",
            "https://user@ntfy.sh/up",
            "ftp://x/",
            "https://ntfy.sh/a b",
            "https://ntfy.sh/a\r\nX: y",
            "",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        for ip in ["1.1.1.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            assert!(public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
    }
}
