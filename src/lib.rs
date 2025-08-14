//! # Actix Route Rate Limiter
//!
//! `Actix Route Rate Limiter` implements the traits
//! necessary to create [middleware](https://actix.rs/docs/middleware/)
//! in the Actix Web framework.
//!
//! This crate can be used to wrap routes with rate limiting logic by defining a duration
//! and number of requests that will be forwarded during that duration.
//!
//! If a quantity of requests exceeds this amount, the middleware will short circuit the request
//! and instead send an `HTTP 429 - Too Many Requests` response with headers describing the rate limit:
//! - `Retry-After` :  the rate-limiting duration, begins at first request received and ends after this elapsed time
//! - `X-RateLimit-Limit` : number of requests allowed for the duration
//! - `X-RateLimit-Remaining` : number of requests remaining for current duration
//! - `X-RateLimit-Reset` : number of seconds remaining in the duration
//!
//!

use actix_service::{Service, Transform};
use actix_web::body::{BoxBody, EitherBody, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{Error, HttpResponse};
use chrono::{DateTime, Duration, Utc};
use futures::future::{ok, Ready};
use log::{info, warn};
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

pub struct RateLimiter {
    pub(crate) limiter: Arc<Mutex<Limiter>>,
}

impl RateLimiter {
    pub fn new(limiter: Arc<Mutex<Limiter>>) -> Self {
        Self { limiter }
    }
}

#[derive(Clone)]
pub struct TimeCount {
    last_request: DateTime<Utc>,
    num_requests: usize,
}

pub struct Limiter {
    pub ip_addresses: HashMap<IpAddr, TimeCount>,
    pub duration: Duration,
    pub num_requests: usize,
}

pub struct LimiterBuilder {
    duration: Duration,
    num_requests: usize,
    cleanup_interval: Option<Duration>,
}

/// Builds a new Limiter with the following defaults:
/// - duration of 1 second 
/// - one request allowed for that duration 
/// - no scheduled cleanup
///  # Example
/// ```
/// use chrono::Duration;
/// use actix_route_rate_limiter::LimiterBuilder;
///
/// let limiter = LimiterBuilder::new()
///         .with_duration(Duration::seconds(20))
///         .with_num_requests(2)
///       //.with_cleanup(Duration::minutes(1))  // optionally configure a cleanup task 
///         .build();
///
/// assert_eq!(limiter.lock().unwrap().duration.num_seconds() , 20);
/// assert_eq!(limiter.lock().unwrap().num_requests, 2);
/// assert!(limiter.lock().unwrap().ip_addresses.is_empty());
/// ```
impl LimiterBuilder {
    /// If no builder methods are used the limiter will default to 1 request/second
    pub fn new() -> Self {
        Self {
            duration: Duration::seconds(1),
            num_requests: 1,
            cleanup_interval: None,
        }
    }

    /// Specifies the duration for which a limit applies
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Specifies the number of requests that will be allowed for the given duration
    pub fn with_num_requests(mut self, num_requests: usize) -> Self {
        self.num_requests = num_requests;
        self
    }

    /// Specifies a cleanup interval after which IPs with inactive limits will be pruned
    pub fn with_cleanup(mut self, run_every: Duration) -> Self {
        self.cleanup_interval = Some(run_every);
        self
    }

    pub fn build(self) -> Arc<Mutex<Limiter>> {
        let ip_addresses = HashMap::new();

        let limiter = Arc::new(Mutex::new(Limiter {
            ip_addresses,
            duration: self.duration,
            num_requests: self.num_requests,
        }));

        if let Some(run_every) = self.cleanup_interval {
            spawn_cleanup_task(&limiter, run_every);
        }

        limiter
    }
}

pub struct RateLimiterMiddleware<S> {
    pub(crate) service: Arc<S>,
    pub(crate) limiter: Arc<Mutex<Limiter>>,
}

impl<S, B> Transform<S, ServiceRequest> for RateLimiter
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    type Response = ServiceResponse<EitherBody<B, BoxBody>>;
    type Error = Error;
    type Transform = RateLimiterMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RateLimiterMiddleware {
            service: Arc::new(service),
            limiter: self.limiter.clone(),
        })
    }
}

impl<S, B> Service<ServiceRequest> for RateLimiterMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    type Response = ServiceResponse<EitherBody<B, BoxBody>>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let limiter = Arc::clone(&self.limiter);
        let service = Arc::clone(&self.service);

        Box::pin(handle_rate_limiting(req, limiter, service))
    }
}

async fn handle_rate_limiting<S, B>(
    req: ServiceRequest,
    limiter: Arc<Mutex<Limiter>>,
    service: Arc<S>,
) -> Result<ServiceResponse<EitherBody<B>>, Error>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    let ip = match req.peer_addr() {
        Some(addr) => addr.ip(),
        None => {
            // peer_Addr only returns None during unit test https://docs.rs/actix-web/latest/actix_web/struct.HttpRequest.html#method.peer_addr
            warn!(
                "Requester socket address was found to be None type and will not be rate limited"
            );
            let res: ServiceResponse<B> = service.call(req).await?;
            return Ok(res.map_into_left_body());
        }
    };

    let now = Utc::now();
    let mut limiter_guard = limiter.lock().unwrap();

    // Obtain immutable limiter properties for comparison
    let duration = limiter_guard.duration;
    let num_requests_limit = limiter_guard.num_requests;

    // Obtain a mutable reference to the TimeCount entry
    let time_count_entry = limiter_guard
        .ip_addresses
        .entry(ip)
        .or_insert_with(|| TimeCount {
            last_request: now,
            num_requests: 0,
        });

    let mut too_many_requests = false;

    if now - time_count_entry.last_request <= duration {
        if time_count_entry.num_requests >= num_requests_limit {
            too_many_requests = true;
        } else {
            time_count_entry.num_requests += 1;
            info!(
                "Incremented request count for {} to {} requests in current duration",
                ip, time_count_entry.num_requests
            );
        }
    } else {
        time_count_entry.last_request = now;
        time_count_entry.num_requests = 1;
        info!("Reset duration and request count for {}", ip)
    }

    if too_many_requests {
        info!("Sending 429 response to {}", ip);

        let last_request_time = time_count_entry.last_request;
        let new_request_count = time_count_entry.num_requests;

        let remaining_time = limiter_guard.duration - (now - last_request_time);
        let limiter_duration_secs = limiter_guard.duration.num_seconds();

        let message = format!(
            "Too many requests. Please try again in {} seconds.",
            remaining_time.num_seconds()
        );

        let too_many_requests_response = HttpResponse::TooManyRequests()
            .content_type("text/plain")
            .insert_header(("Retry-After", limiter_duration_secs.to_string()))
            .insert_header(("X-RateLimit-Limit", limiter_guard.num_requests.to_string()))
            .insert_header((
                "X-RateLimit-Remaining",
                (limiter_guard.num_requests - new_request_count).to_string(),
            ))
            .insert_header((
                "X-RateLimit-Reset",
                remaining_time.num_seconds().to_string(),
            ))
            .body(message);

        return Ok(
            ServiceResponse::new(req.request().clone(), too_many_requests_response)
                .map_into_boxed_body()
                .map_into_right_body(),
        );
    }

    info!("Forwarding request from {} to {}", ip, req.path());
    let res: ServiceResponse<B> = service.call(req).await?;
    Ok(res.map_into_left_body())
}

/// Spawns a best-effort background task that periodically prunes stale
/// rate-limit entries from the in-memory map.
///
/// Behavior
/// - Runs every `run_every` using a Tokio interval.
/// - Removes IP entries that have been idle for more than `limiter.duration`.
///
/// Lifecycle
/// - Holds only a `Weak` reference to the limiter; when the limiter is dropped,
///   the task stops automatically on the next tick and no resources are leaked.
///
/// Scheduling
/// - Uses `MissedTickBehavior::Skip` so ticks may be skipped under load; this is
///   acceptable because cleanup is best-effort and will catch up later.
///
/// Concurrency
/// - Locks the limiter `Mutex` briefly and performs an O(n) `retain` over the
///   IP map. The lock is released immediately after pruning.
/// - If the lock is poisoned, the inner value is recovered and cleanup proceeds.
fn spawn_cleanup_task(limiter: &Arc<Mutex<Limiter>>, run_every: Duration) {
    let weak: Weak<Mutex<Limiter>> = Arc::downgrade(limiter);
    tokio::spawn(async move {
        // Convert chrono::Duration to std::time::Duration with sub-second precision.
        // Using num_seconds() would truncate to 0 for sub-second values, causing a panic
        // in tokio::time::interval. to_std() preserves the full precision.
        let std_run_every = match run_every.to_std() {
            Ok(d) if d > std::time::Duration::from_nanos(0) => d,
            Ok(_) | Err(_) => std::time::Duration::from_millis(1), // fallback to a minimal non-zero duration
        };
        let mut ticker = tokio::time::interval(std_run_every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let Some(limiter_arc) = weak.upgrade() else {
                break;
            };

            let now = Utc::now();

            let removed = {
                let mut guard = match limiter_arc.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };

                let duration = guard.duration;
                let before = guard.ip_addresses.len();
                guard
                    .ip_addresses
                    .retain(|_ip, tc| now - tc.last_request <= duration);
                before - guard.ip_addresses.len()
            };

            if removed > 0 {
                info!("Cleanup removed {} stale rate-limit entries", removed);
            }
        }
    });
}
