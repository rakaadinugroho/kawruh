//! In-memory login throttling. After `max_failures` failed logins for the same
//! username or the same client IP within `window` (counted from the first
//! failure), further attempts are refused until that window has passed. A
//! successful login clears the username's failures.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts};
use http::request::Parts;

/// Most usernames + IPs tracked at once; beyond it, expired entries are
/// pruned and then the oldest dropped, so memory stays bounded.
const MAX_KEYS: usize = 10_000;

pub struct LoginLimiter {
    max_failures: u32,
    window: Duration,
    max_keys: usize,
    /// `user:<name>` / `ip:<addr>` → (first failure of the window, failures).
    entries: Mutex<HashMap<String, (Instant, u32)>>,
}

fn keys(username: &str, ip: Option<IpAddr>) -> Vec<String> {
    let mut keys = vec![format!("user:{username}")];
    if let Some(ip) = ip {
        keys.push(format!("ip:{ip}"));
    }
    keys
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window: Duration) -> LoginLimiter {
        LoginLimiter {
            max_failures,
            window,
            max_keys: MAX_KEYS,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, u32)>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// How long until an attempt for `username` from `ip` is allowed again,
    /// when either is blocked.
    pub fn blocked(&self, username: &str, ip: Option<IpAddr>) -> Option<Duration> {
        self.blocked_at(username, ip, Instant::now())
    }

    fn blocked_at(&self, username: &str, ip: Option<IpAddr>, now: Instant) -> Option<Duration> {
        let entries = self.lock();
        keys(username, ip)
            .iter()
            .filter_map(|k| entries.get(k))
            .filter(|(_, failures)| *failures >= self.max_failures)
            .filter_map(|(start, _)| (*start + self.window).checked_duration_since(now))
            .filter(|left| !left.is_zero())
            .max()
    }

    pub fn record_failure(&self, username: &str, ip: Option<IpAddr>) {
        self.record_failure_at(username, ip, Instant::now());
    }

    fn record_failure_at(&self, username: &str, ip: Option<IpAddr>, now: Instant) {
        let mut entries = self.lock();
        for key in keys(username, ip) {
            if !entries.contains_key(&key) && entries.len() >= self.max_keys {
                entries.retain(|_, (start, _)| now.duration_since(*start) < self.window);
                if entries.len() >= self.max_keys
                    && let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, (start, _))| *start)
                        .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            let entry = entries.entry(key).or_insert((now, 0));
            if now.duration_since(entry.0) >= self.window {
                *entry = (now, 0);
            }
            entry.1 = entry.1.saturating_add(1);
        }
    }

    /// Forgets a username's failures (after it logged in successfully).
    pub fn clear(&self, username: &str) {
        self.lock().remove(&format!("user:{username}"));
    }
}

/// The client's IP: `CF-Connecting-IP` when present, else the first
/// `X-Forwarded-For` entry, else the socket peer. A client that reaches the
/// server directly can spoof those headers, so only the per-username limit is
/// reliable then.
pub struct ClientIp(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip(parts)))
    }
}

pub fn client_ip(parts: &Parts) -> Option<IpAddr> {
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
    header("cf-connecting-ip")
        .and_then(|v| v.trim().parse().ok())
        .or_else(|| {
            header("x-forwarded-for")
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse().ok())
        })
        .or_else(|| {
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)));
    const OTHER_IP: Option<IpAddr> = Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2)));

    #[test]
    fn blocks_after_max_failures_until_the_window_passes() {
        let l = LoginLimiter::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        for i in 0..3 {
            assert_eq!(l.blocked_at("alice", IP, t0), None, "attempt {i}");
            l.record_failure_at("alice", IP, t0);
        }
        assert_eq!(
            l.blocked_at("alice", None, t0 + Duration::from_secs(10)),
            Some(Duration::from_secs(50))
        );
        // The IP is blocked for other usernames too.
        assert!(l.blocked_at("bob", IP, t0).is_some());
        assert_eq!(l.blocked_at("bob", OTHER_IP, t0), None);
        assert_eq!(
            l.blocked_at("alice", IP, t0 + Duration::from_secs(60)),
            None
        );
        // A failure after the window starts a new one.
        l.record_failure_at("alice", IP, t0 + Duration::from_secs(61));
        assert_eq!(
            l.blocked_at("alice", IP, t0 + Duration::from_secs(61)),
            None
        );
    }

    #[test]
    fn success_clears_the_username_but_not_the_ip() {
        let l = LoginLimiter::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        l.record_failure_at("alice", IP, t0);
        l.record_failure_at("alice", OTHER_IP, t0);
        assert!(l.blocked_at("alice", None, t0).is_some());
        l.clear("alice");
        assert_eq!(l.blocked_at("alice", None, t0), None);
        l.record_failure_at("carol", IP, t0);
        assert!(l.blocked_at("dave", IP, t0).is_some());
    }

    #[test]
    fn memory_stays_bounded() {
        let mut l = LoginLimiter::new(1, Duration::from_secs(60));
        l.max_keys = 4;
        let t0 = Instant::now();
        l.record_failure_at("old", None, t0);
        for i in 0..20u8 {
            let ip = Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, i)));
            l.record_failure_at(&format!("u{i}"), ip, t0 + Duration::from_secs(1));
        }
        assert!(l.lock().len() <= 4);
        // Expired entries go first.
        let l = LoginLimiter {
            max_keys: 2,
            ..LoginLimiter::new(1, Duration::from_secs(60))
        };
        l.record_failure_at("a", None, t0);
        l.record_failure_at("b", None, t0 + Duration::from_secs(30));
        l.record_failure_at("c", None, t0 + Duration::from_secs(70));
        let entries = l.lock();
        assert!(entries.contains_key("user:b") && entries.contains_key("user:c"));
    }

    fn parts(headers: &[(&str, &str)], peer: Option<&str>) -> Parts {
        let mut req = http::Request::builder().uri("/");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let (mut parts, _) = req.body(()).unwrap().into_parts();
        if let Some(peer) = peer {
            parts
                .extensions
                .insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        }
        parts
    }

    #[test]
    fn client_ip_prefers_cloudflare_then_forwarded_then_peer() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let peer = Some("127.0.0.1:5000");
        assert_eq!(
            client_ip(&parts(
                &[
                    ("CF-Connecting-IP", "2001:db8::1"),
                    ("X-Forwarded-For", "203.0.113.5")
                ],
                peer
            )),
            ip("2001:db8::1")
        );
        assert_eq!(
            client_ip(&parts(
                &[("X-Forwarded-For", " 203.0.113.5 , 10.0.0.1")],
                peer
            )),
            ip("203.0.113.5")
        );
        assert_eq!(
            client_ip(&parts(&[("X-Forwarded-For", "garbage")], peer)),
            ip("127.0.0.1")
        );
        assert_eq!(client_ip(&parts(&[], None)), None);
    }
}
