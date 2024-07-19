use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Duration, Utc};
use actix_service::Transform;
use actix_web::dev::ServiceRequest;
use actix_web::{Error, HttpResponse};
use futures::future::{ok, Ready};
use std::task::{Context, Poll};
use actix_web::body::BoxBody;

pub struct Limiter {
    pub ip_addresses: HashMap<String, (DateTime<Utc>, usize)>,
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

impl<S> Transform<S, ServiceRequest> for RateLimiter
where
    S: Service<ServiceRequest, Response=ServiceResponse<BoxBody>, Error=Error> + 'static,
    S::Future: 'static,
{
    type Response = ServiceResponse<BoxBody>;
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

pub(crate) async fn handle_rate_limiting<S>(
    req: ServiceRequest,
    limiter: Arc<Mutex<Limiter>>,
    service: Arc<S>,
) -> Result<ServiceResponse<BoxBody>, Error>
where
    S: Service<ServiceRequest, Response=ServiceResponse<BoxBody>, Error=Error>,
{
    let bad_response = HttpResponse::BadRequest().finish().map_into_boxed_body();
    let too_many_requests = HttpResponse::TooManyRequests().finish().map_into_boxed_body();
    let ip = match req.peer_addr().map(|addr| addr.ip().to_string()) {
        Some(ip) => ip,
        None => return Ok(req.into_response(bad_response)),
    };

    let now = Utc::now();
    {
        let mut limiter = limiter.lock().unwrap();
        let (last_request_time, request_count) = limiter.ip_addresses.entry(ip.clone())
            .or_insert((now, 1));

        if now - *last_request_time <= Duration::seconds(20) {
            if *request_count >= 2 {
                return Ok(req.into_response(too_many_requests));
            } else {
                *request_count += 1;
            }
        } else {
            // Reset time and count after 20 seconds
            *last_request_time = now;
            *request_count = 1;
        }
    }

    let res = service.call(req).await?;
    Ok(res)
}

impl<S> Service<ServiceRequest> for RateLimiterMiddleware<S>
where
    S: Service<ServiceRequest, Response=ServiceResponse<BoxBody>, Error=Error> + 'static,
    S::Future: 'static,
{
    type Response = ServiceResponse<BoxBody>;
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