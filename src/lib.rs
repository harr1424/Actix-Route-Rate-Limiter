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

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Duration, Utc};
use actix_service::{Service, Transform};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{Error, HttpResponse};
use futures::future::{ok, Ready};
use std::task::{Context, Poll};
use actix_web::body::{BoxBody, EitherBody, MessageBody};
use log::{info, warn};

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
}

/// Builds a new Limiter. If with_duration() is not used
/// the default duration will be one second. If with_num_requests()
/// is not used the default value of one request will be used.
///  # Example
/// ```
/// use chrono::Duration;
/// use actix_route_rate_limiter::LimiterBuilder;
///
/// let limiter = LimiterBuilder::new()
///         .with_duration(Duration::seconds(20))
///         .with_num_requests(2)
///         .build();
///
/// assert_eq!(limiter.lock().unwrap().duration.num_seconds() , 20);
/// assert_eq!(limiter.lock().unwrap().num_requests, 2);
/// assert!(limiter.lock().unwrap().ip_addresses.is_empty());
/// ```
impl LimiterBuilder {
    /// If no builder methods are used the limiter will default to
    /// forwarding one request per second.
    pub fn new() -> Self {
        Self {
            duration: Duration::seconds(1),
            num_requests: 1,
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

    pub fn build(self) -> Arc<Mutex<Limiter>> {
        let ip_addresses = HashMap::new();

        Arc::new(Mutex::new(Limiter {
            ip_addresses,
            duration: self.duration,
            num_requests: self.num_requests,
        }))
    }
}

 pub struct RateLimiter {
    pub(crate) limiter: Arc<Mutex<Limiter>>,
}

impl RateLimiter {
    pub fn new(limiter: Arc<Mutex<Limiter>>) -> Self {
        Self { limiter }
    }
}

 pub struct RateLimiterMiddleware<S> {
    pub(crate) service: Arc<S>,
    pub(crate) limiter: Arc<Mutex<Limiter>>,
}

impl<S, B> Transform<S, ServiceRequest> for RateLimiter
where
    S: Service<ServiceRequest, Response=ServiceResponse<B>, Error=Error> + 'static,
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
    S: Service<ServiceRequest, Response=ServiceResponse<B>, Error=Error> + 'static,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    type Response = ServiceResponse<EitherBody<B, BoxBody>>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output=Result<Self::Response, Self::Error>>>>;

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
    S: Service<ServiceRequest, Response=ServiceResponse<B>, Error=Error>,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    let ip = match req.peer_addr() {
        Some(addr) => addr.ip(),
        None => {
            // peer_Addr only returns None during unit test https://docs.rs/actix-web/latest/actix_web/struct.HttpRequest.html#method.peer_addr
            warn!("Requester socket address was found to be None type and will not be rate limited");
            let res: ServiceResponse<B> = service.call(req).await?;
            return Ok(res.map_into_left_body());
        }
    };

    let now = Utc::now();
    let mut limiter = limiter.lock().unwrap();

    // immutable local copy of hashmap values
    let time_count = {
        limiter.ip_addresses.entry(ip.clone()).or_insert(TimeCount {
            last_request: now,
            num_requests: 0,
        }).clone()
    };

    let last_request_time = time_count.last_request;
    let request_count = time_count.num_requests;

    let mut too_many_requests = false;
    let mut new_last_request_time = last_request_time;
    let mut new_request_count = request_count;

    let limiter_duration_secs = limiter.duration.num_seconds();

    if now - last_request_time <= Duration::seconds(limiter_duration_secs) {
        if request_count >= limiter.num_requests {
            too_many_requests = true;
        } else {
            new_request_count += 1;
            info!("Incremented request count for {} to {} requests in current duration", ip, new_request_count);
        }
    } else {
        new_last_request_time = now;
        new_request_count = 1;
        info!("Reset duration and request count for {}", ip)
    }

    // mutable borrow of hashmap values
    let entry = limiter.ip_addresses.entry(ip.clone()).or_insert(TimeCount { last_request: now, num_requests: 0 });
    entry.last_request = new_last_request_time;
    entry.num_requests = new_request_count;


    if too_many_requests {
        info!("Sending 429 response to {}", ip);
        let remaining_time = limiter.duration - (now - last_request_time);
        let message = format!("Too many requests. Please try again in {} seconds.", remaining_time.num_seconds().to_string());

        let too_many_requests_response = HttpResponse::TooManyRequests()
            .content_type("text/plain")
            .insert_header(("Retry-After", limiter_duration_secs.to_string()))
            .insert_header(("X-RateLimit-Limit", limiter.num_requests.to_string()))
            .insert_header(("X-RateLimit-Remaining", (limiter.num_requests - new_request_count).to_string()))
            .insert_header(("X-RateLimit-Reset", remaining_time.num_seconds().to_string()))
            .body(message);
        return Ok(ServiceResponse::new(req.request().clone(), too_many_requests_response)
            .map_into_boxed_body()
            .map_into_right_body());
    }

    info!("Forwarding request from {} to {}", ip, req.path());
    let res: ServiceResponse<B> = service.call(req).await?;
    Ok(res.map_into_left_body())
}