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
    pub duration: Duration,
    pub num_requests: usize,
}

pub struct LimiterBuilder {
    duration: Duration,
    num_requests: usize,
}

impl LimiterBuilder {
    pub fn new() -> Self {
        Self {
            duration: Duration::days(1),
            num_requests: 1,
        }
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

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
    let ip = match req.peer_addr() {
        Some(addr) => addr.ip().to_string(),
        None => {
            let bad_request_response = HttpResponse::BadRequest()
                .content_type("text/html")
                .body("Missing IP address.");
            return Ok(ServiceResponse::new(req.request().clone(), bad_request_response)
                .map_into_boxed_body()
                .map_into_right_body());
        }
    };
    let now = Utc::now();

    let (last_request_time, request_count) = {
        let mut limiter = limiter.lock().unwrap();
        limiter.ip_addresses.entry(ip.clone()).or_insert((now, 0)).clone()
    };

    println!("IP: {} - Last Request Time: {}, Request Count: {}", ip, last_request_time, request_count);

    let mut too_many_requests = false;
    let mut new_last_request_time = last_request_time;
    let mut new_request_count = request_count;

    let limiter_duration_secs = limiter.lock().unwrap().duration.num_seconds();

    if now - last_request_time <= Duration::seconds(limiter_duration_secs) {
        if request_count >= limiter.lock().unwrap().num_requests {
            too_many_requests = true;
        } else {
            new_request_count += 1;
            println!("IP: {} - Incremented Request Count: {}", ip, new_request_count);
        }
    } else {
        // Reset time and count
        new_last_request_time = now;
        new_request_count = 1;
        println!("IP: {} - Reset Request Count and Time", ip);
    }

    {
        let mut limiter = limiter.lock().unwrap();
        let entry = limiter.ip_addresses.entry(ip.clone()).or_insert((now, 0));
        entry.0 = new_last_request_time;
        entry.1 = new_request_count;
    }

    if too_many_requests {
        println!("IP: {} - Too Many Requests", ip);
        let too_many_requests_response = HttpResponse::TooManyRequests()
            .content_type("text/html")
            .insert_header(("Retry-After", limiter_duration_secs.to_string()))
            .body("Too many requests. Please try again later.");
        return Ok(ServiceResponse::new(req.request().clone(), too_many_requests_response)
            .map_into_boxed_body()
            .map_into_right_body());
    }

    let res: ServiceResponse<B> = service.call(req).await?;
    Ok(res.map_into_left_body())
}





