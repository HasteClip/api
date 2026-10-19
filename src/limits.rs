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

/// Who's asking: the address the proxy (Coolify's Traefik) saw, which it
/// adds last to `X-Forwarded-For`, or whoever connected when there's no
/// proxy. Anything before the last entry is what the client claims, so it's
/// ignored. IPv6 addresses count by their /64, which one person usually has
/// all of.
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
        let ip = forwarded
            .or_else(|| req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip()))
            .ok_or(GovernorError::UnableToExtractKey)?;
        Ok(match ip {
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            v4 => v4,
        })
    }
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
        let mut req = Request::builder();
        for h in headers {
            req = req.header("x-forwarded-for", *h);
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
}
