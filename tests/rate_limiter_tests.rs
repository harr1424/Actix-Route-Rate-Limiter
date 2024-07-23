use actix_web::{web, App, HttpServer, HttpResponse};
use actix_web::test::{init_service, call_service, TestRequest};
use std::sync::{Arc, Mutex, RwLock};
use actix_route_rate_limiter::{Limiter, LimiterBuilder, RateLimiter};
use actix_rt::time::sleep;
use std::time::Duration as StdDuration;
use chrono::Duration;
use tokio::task::spawn;
use reqwest::{Client, StatusCode};
use tokio::sync::Barrier;
use std::net::{TcpListener, IpAddr, Ipv4Addr, SocketAddr};
use lazy_static::lazy_static;
use log::{LevelFilter, Record};

lazy_static! {
    static ref LOGS: RwLock<Vec<String>> = RwLock::new(vec![]);
}

struct TestLogger;

impl log::Log for TestLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut logs = LOGS.write().unwrap();
            logs.push(format!("{}", record.args()));
        }
    }

    fn flush(&self) {}
}

static LOGGER: TestLogger = TestLogger;

fn init_test_logger() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(LevelFilter::Info);
    LOGS.write().unwrap().clear();
}

async fn start_server(limiter: Arc<Mutex<Limiter>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(|| HttpResponse::Ok()))
    })
        .listen(listener)
        .unwrap()
        .run();

    spawn(server);
    port
}

#[actix_rt::test]
async fn test_rate_limiter_allows_two_requests() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build();

    let limiter = Arc::new(limiter);

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // Simulate two requests from the same IP within 20 seconds
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Third request within 20 seconds should be rate-limited
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());

    // Attempt a fourth request immediately (Expected to fail also)
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Incremented request count")));
    assert!(logs.iter().any(|log| log.contains("Sending 429 response to")));
}

#[actix_rt::test]
async fn test_rate_limiter_allows_after_duration() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build();

    let limiter = Arc::new(limiter);

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // First request should succeed
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second request should succeed
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait for more than 20 seconds
    sleep(StdDuration::from_secs(21)).await;

    // Third request should also succeed after waiting
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Fourth request after waiting should succeed
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Reset duration and request count")));
}

#[actix_rt::test]
async fn test_rate_limiter_with_different_ips() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build();

    let service = init_service(
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok)),
    )
        .await;

    // First request from IP 127.0.0.1
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    // First request from IP 127.0.0.2
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    // Second request from IP 127.0.0.1
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    // Third request from IP 127.0.0.1 within 20 seconds should be rate-limited
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Incremented request count for 127.0.0.1")));
    assert!(logs.iter().any(|log| log.contains("Sending 429 response to 127.0.0.1")));
}

#[actix_rt::test]
async fn test_rate_limiter_handles_missing_ip() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build();

    let service = init_service(
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok)),
    )
        .await;

    let req = TestRequest::default().to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Requester socket address was found to be None type and will not be rate limited")));
}


#[actix_rt::test]
async fn test_rate_limiter_multi_threaded() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let barrier = Arc::new(Barrier::new(4));
    let client = Arc::new(Client::new());
    let mut handles = vec![];

    for _ in 0..4 {
        let barrier = Arc::clone(&barrier);
        let client = Arc::clone(&client);
        let handle = spawn(async move {
            barrier.wait().await;
            let resp = client.get(&format!("http://127.0.0.1:{}/", port))
                .send()
                .await
                .unwrap();
            resp.status()
        });
        handles.push(handle);
    }

    let mut success_count = 0;
    let mut rate_limited_count = 0;

    for handle in handles {
        let status = handle.await.unwrap();
        if status == StatusCode::OK {
            success_count += 1;
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            rate_limited_count += 1;
        }
    }

    assert_eq!(success_count, 2);
    assert_eq!(rate_limited_count, 2);

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Incremented request count")));
    assert!(logs.iter().any(|log| log.contains("Sending 429 response to")));
}
