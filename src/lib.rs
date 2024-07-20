use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Duration, Utc};
use actix_service::{Service, Transform};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{Error, HttpResponse};
use futures::future::{ok, Ready};
use std::task::{Context, Poll};
use actix_web::body::{BoxBody, EitherBody, MessageBody};


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

impl<S, B> Transform<S, ServiceRequest> for RateLimiter
where
    S: Service<ServiceRequest, Response=ServiceResponse<B>, Error=Error> + 'static,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    type Response=ServiceResponse<EitherBody<B, BoxBody>>;
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
    type Response=ServiceResponse<EitherBody<B, BoxBody>>;
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

pub async fn handle_rate_limiting<S, B>(
    req: ServiceRequest,
    limiter: Arc<Mutex<Limiter>>,
    service: Arc<S>,
) -> Result<ServiceResponse<EitherBody<B>>, Error>
where
    S: Service<ServiceRequest, Response=ServiceResponse<B>, Error=Error>,
    S::Future: 'static,
    B: 'static + MessageBody,
{
    let ip = match req.peer_addr().map(|addr| addr.ip().to_string()) {
        Some(ip) => ip,
        None => {
            let bad_request_response = HttpResponse::BadRequest().finish();
            return Ok(ServiceResponse::new(req.request().clone(), bad_request_response)
                .map_into_boxed_body()
                .map_into_right_body());
        }
    };

    println!("Request from IP: {}", ip);

    let now = Utc::now();
    {
        let mut limiter = limiter.lock().unwrap();
        let (last_request_time, request_count) = limiter.ip_addresses.entry(ip.clone())
            .or_insert((now, 0));

        println!("IP: {} - Last Request Time: {}, Request Count: {}", ip, last_request_time, request_count);

        if now - *last_request_time <= Duration::seconds(20) {
            if *request_count >= 2 {
                println!("IP: {} - Too Many Requests", ip);
                let too_many_requests_response = HttpResponse::TooManyRequests().finish();
                return Ok(ServiceResponse::new(req.request().clone(), too_many_requests_response)
                    .map_into_boxed_body()
                    .map_into_right_body());
            } else {
                *request_count += 1;
                println!("IP: {} - Incremented Request Count: {}", ip, request_count);
            }
        } else {
            // Reset time and count after 20 seconds
            *last_request_time = now;
            *request_count = 1;
            println!("IP: {} - Reset Request Count and Time", ip);
        }
    }

    let res : ServiceResponse<B> = service.call(req).await?;
    Ok(res.map_into_left_body())
}





