//! Rate limits, per address, kept in memory (nothing about addresses is
//! written anywhere).

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use governor::middleware::NoOpMiddleware;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

/// Who's asking. Cloudflare is in front, so the address Coolify's Traefik
/// saw (which it adds last to `X-Forwarded-For`) is one of Cloudflare's,
/// shared by everyone near the same Cloudflare data centre. When it is,
/// the visitor's own address is the one Cloudflare sends as
/// `CF-Connecting-IP`. From anywhere else, that header is just a claim, so
/// it counts only from Cloudflare's addresses; without Cloudflare (or
/// running locally) it's the address Traefik saw, or whoever connected.
/// Anything before the last `X-Forwarded-For` entry is what the client
/// claims, so it's ignored. IPv6 addresses count by their /64, which one
/// person usually has all of.
#[derive(Clone, Copy)]
pub struct ClientIp;

impl KeyExtractor for ClientIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<IpAddr, GovernorError> {
        let forwarded = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .next_back()
            .and_then(|s| s.trim().parse().ok());
        let proxy_saw = forwarded
            .or_else(|| req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip()))
            .ok_or(GovernorError::UnableToExtractKey)?;
        let visitor = || req.headers().get("cf-connecting-ip")?.to_str().ok()?.trim().parse().ok();
        let ip = if is_cloudflare(proxy_saw) { visitor().unwrap_or(proxy_saw) } else { proxy_saw };
        Ok(match ip {
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            v4 => v4,
        })
    }
}

/// Cloudflare's addresses, from cloudflare.com/ips (the same since 2021).
/// One it adds later counts as a single visitor until it's listed here.
const CLOUDFLARE: [&str; 22] = [
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

fn is_cloudflare(ip: IpAddr) -> bool {
    // An IPv4 address that arrived as IPv6 (::ffff:a.b.c.d) is still IPv4.
    let ip = ip.to_canonical();
    CLOUDFLARE.iter().any(|range| {
        let (net, bits) = range.split_once('/').expect("a range");
        let bits: u32 = bits.parse().expect("a prefix length");
        match (ip, net.parse().expect("an address")) {
            (IpAddr::V4(ip), IpAddr::V4(net)) => {
                (u32::from(ip) ^ u32::from(net)).checked_shr(32 - bits).unwrap_or(0) == 0
            }
            (IpAddr::V6(ip), IpAddr::V6(net)) => {
                (u128::from(ip) ^ u128::from(net)).checked_shr(128 - bits).unwrap_or(0) == 0
            }
            _ => false,
        }
    })
}

type Config = GovernorConfig<ClientIp, NoOpMiddleware>;

/// Up to `burst` requests at once, then one more every `every`.
pub fn limit(every: Duration, burst: u32) -> Arc<Config> {
    Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(ClientIp)
            .period(every)
            .burst_size(burst)
            .finish()
            .expect("a valid rate limit"),
    )
}

pub fn layer(config: &Arc<Config>) -> GovernorLayer<ClientIp, NoOpMiddleware, Body> {
    GovernorLayer::new(config.clone()).error_handler(too_many)
}

/// Forget addresses that haven't been seen in a while, every minute.
pub fn spawn_cleanup(configs: Vec<Arc<Config>>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            for c in &configs {
                c.limiter().retain_recent();
            }
        }
    });
}

fn too_many(e: GovernorError) -> Response<Body> {
    let message = match e {
        GovernorError::TooManyRequests { wait_time, .. } => {
            format!(
                "Too many requests. Try again in {}.",
                if wait_time > 90 {
                    format!("{} minutes", wait_time.div_ceil(60))
                } else {
                    format!("{wait_time} seconds")
                }
            )
        }
        _ => "Too many requests.".into(),
    };
    (StatusCode::TOO_MANY_REQUESTS, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(headers: &[&str]) -> IpAddr {
        from(headers, None)
    }

    /// The key for a request with these `X-Forwarded-For` headers and,
    /// if given, `CF-Connecting-IP`.
    fn from(forwarded: &[&str], cf: Option<&str>) -> IpAddr {
        let mut req = Request::builder();
        for h in forwarded {
            req = req.header("x-forwarded-for", *h);
        }
        if let Some(cf) = cf {
            req = req.header("cf-connecting-ip", cf);
        }
        ClientIp.extract(&req.body(()).unwrap()).unwrap()
    }

    #[test]
    fn the_proxys_entry_counts() {
        assert_eq!(ip(&["203.0.113.9"]), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(ip(&["1.2.3.4, 203.0.113.9"]), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(ip(&["1.2.3.4", "203.0.113.9"]), "203.0.113.9".parse::<IpAddr>().unwrap());
        assert_eq!(ip(&["2001:db8:1:2:3:4:5:6"]), "2001:db8:1:2::".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn behind_cloudflare_the_visitor_counts() {
        let visitor = "203.0.113.9".parse::<IpAddr>().unwrap();
        assert_eq!(from(&["203.0.113.9, 162.158.1.2"], Some("203.0.113.9")), visitor);
        assert_eq!(from(&["188.114.97.2"], Some(" 203.0.113.9 ")), visitor);
        assert_eq!(
            from(&["2a06:98c1:3121::2"], Some("2001:db8:1:2:3:4:5:6")),
            "2001:db8:1:2::".parse::<IpAddr>().unwrap()
        );
        // Cloudflare's own address, when it doesn't say whose request it is.
        assert_eq!(from(&["162.158.1.2"], None), "162.158.1.2".parse::<IpAddr>().unwrap());
        assert_eq!(from(&["162.158.1.2"], Some("nonsense")), "162.158.1.2".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn only_cloudflare_is_believed() {
        // Straight to the server, claiming to be someone else.
        assert_eq!(from(&["198.51.100.7"], Some("203.0.113.9")), "198.51.100.7".parse::<IpAddr>().unwrap());
        assert_eq!(
            from(&["162.158.1.2, 198.51.100.7"], Some("203.0.113.9")),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn cloudflare_is_recognised() {
        for ip in [
            "173.245.48.0",
            "173.245.63.255",
            "104.16.0.1",
            "104.23.255.255",
            "188.114.97.2",
            "::ffff:162.158.1.2",
            "2606:4700::1",
            "2a06:98c7:ffff::1",
        ] {
            assert!(is_cloudflare(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["173.245.64.0", "104.15.255.255", "2.28.103.251", "127.0.0.1", "2606:4701::1", "2a06:98c8::1", "::1"]
        {
            assert!(!is_cloudflare(ip.parse().unwrap()), "{ip}");
        }
    }
}
