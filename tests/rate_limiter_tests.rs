use actix_web::{web, App, HttpServer, HttpResponse};
use actix_web::test::{init_service, call_service, TestRequest};
use rand::Rng;
use std::sync::{Arc, Mutex, RwLock};
use actix_route_rate_limiter::{Limiter, LimiterBuilder, RateLimiter};
use actix_rt::time::sleep;
use std::time::Duration as StdDuration;
use chrono::Duration;
use tokio::task::spawn;
use reqwest::{Client, StatusCode};
use tokio::sync::Barrier;
use std::net::{TcpListener, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use lazy_static::lazy_static;
use log::{LevelFilter, Record};
use serial_test::serial;

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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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
#[serial]
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

#[actix_rt::test]
#[serial]
async fn test_rate_limiter_high_volume_concurrency() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(10))
        .with_num_requests(100) 
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let barrier = Arc::new(Barrier::new(1000)); // Simulate 1000 concurrent threads
    let client = Arc::new(Client::new());
    let mut handles = vec![];

    for _ in 0..1000 {
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

    // Since 100 requests are allowed, we expect exactly 100 successful requests
    assert_eq!(success_count, 100);
    assert_eq!(rate_limited_count, 900); // The rest should be rate limited
}

#[actix_rt::test]
#[serial]
async fn test_rate_limiter_random_ips() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(20))
        .with_num_requests(2)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Arc::new(Client::new());
    let mut rng = rand::thread_rng();
    
    for _ in 0..50 {
        // Generate random IP
        let random_ip = Ipv4Addr::new(rng.gen::<u8>(), rng.gen::<u8>(), rng.gen::<u8>(), rng.gen::<u8>());
        let resp = client.get(&format!("http://127.0.0.1:{}/", port))
            .header("X-Forwarded-For", random_ip.to_string()) // Simulate random IPs
            .send()
            .await
            .unwrap();

        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::TOO_MANY_REQUESTS);
    }

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Incremented request count")));
}

#[actix_rt::test]
#[serial]
async fn test_cleanup_allows_new_window_after_removal() {
    init_test_logger();

    // Small window to exercise cleanup quickly
    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::milliseconds(300))
        .with_num_requests(1)
        .with_cleanup(Duration::milliseconds(300))
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // First request OK
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second immediate request should be 429
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Wait long enough for threshold (2x=600ms) and at least one cleanup tick
    sleep(StdDuration::from_millis(1000)).await;

    // After cleanup removed the stale entry, next request should be OK again
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Cleanup removed")),
        "expected cleanup log entry not found; logs: {:?}", &*logs);
}

#[actix_rt::test]
#[serial]
async fn test_cleanup_task_does_not_keep_limiter_alive() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::milliseconds(200))
        .with_num_requests(5)
        .build();

    // Initial strong count is 1
    assert_eq!(Arc::strong_count(&limiter), 1);
    {
        // Construct RateLimiter which spawns cleanup using Weak
        let rl = RateLimiter::new(Arc::clone(&limiter));
        assert_eq!(Arc::strong_count(&limiter), 2);
        drop(rl);
    }
    // After dropping RateLimiter, strong count returns to 1
    assert_eq!(Arc::strong_count(&limiter), 1);

    // Give the background task time to tick; it should not resurrect a strong ref
    sleep(StdDuration::from_millis(500)).await;
    assert_eq!(Arc::strong_count(&limiter), 1);
}

#[actix_rt::test]
#[serial]
async fn test_cleanup_removes_stale_entries_logs() {
    init_test_logger();

    // Small duration to make cleanup quick: duration=200ms, cleanup interval=400ms, threshold=400ms
    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::milliseconds(200))
        .with_num_requests(10)
        .with_cleanup(Duration::milliseconds(400))
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // Create one entry
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait long enough for idle time > threshold (400ms) and for cleanup interval to tick (>=400ms)
    sleep(StdDuration::from_millis(1100)).await;

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Cleanup removed 1 stale rate-limit entries")),
        "expected cleanup to remove exactly 1 entry; logs: {:?}", &*logs);
}

#[actix_rt::test]
#[serial]
async fn test_429_headers_are_sane() {
    init_test_logger();

    // Allow 1 per 5 seconds; second request should 429 with coherent headers
    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(5))
        .with_num_requests(1)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Static headers
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "5");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "1");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");

    // Reset should be an integer within [0, 5]
    let reset = resp.headers().get("X-RateLimit-Reset").unwrap().to_str().unwrap();
    let reset_secs: i64 = reset.parse().unwrap_or(-1);
    assert!(0 <= reset_secs && reset_secs <= 5, "reset header out of range: {}", reset_secs);
}

#[actix_rt::test]
#[serial]
async fn test_exact_limit_then_429_headers_limit3() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(2))
        .with_num_requests(3)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    for _ in 0..3 {
        let resp = client.get(&format!("http://127.0.0.1:{}/", port))
            .send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // 4th should 429
    let resp = client.get(&format!("http://127.0.0.1:{}/", port))
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "3");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    let reset = resp.headers().get("X-RateLimit-Reset").unwrap().to_str().unwrap();
    let reset_secs: i64 = reset.parse().unwrap_or(-1);
    assert!(0 <= reset_secs && reset_secs <= 2);
}

#[actix_rt::test]
#[serial]
async fn test_within_duration_stays_blocked_until_expired() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::milliseconds(300))
        .with_num_requests(1)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // First OK
    let resp = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Immediate second should 429
    let resp = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Still within duration
    sleep(StdDuration::from_millis(250)).await;
    let resp = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Past duration now
    sleep(StdDuration::from_millis(200)).await; // total ~450ms > 300ms
    let resp = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let logs = LOGS.read().unwrap();
    assert!(logs.iter().any(|log| log.contains("Reset duration and request count")));
}

#[actix_rt::test]
#[serial]
async fn test_per_ip_isolation_under_concurrency() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(2))
        .with_num_requests(1)
        .build();

    let service = init_service(
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok)),
    ).await;

    let ip1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127,0,0,1)), 11111);
    let ip2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127,0,0,2)), 22222);

    // First requests from each IP concurrently
    let f1 = async {
        let req = TestRequest::default().peer_addr(ip1).to_request();
        call_service(&service, req).await.status()
    };
    let f2 = async {
        let req = TestRequest::default().peer_addr(ip2).to_request();
        call_service(&service, req).await.status()
    };

    let (s1, s2) = futures::future::join(f1, f2).await;
    assert_eq!(s1, actix_web::http::StatusCode::OK);
    assert_eq!(s2, actix_web::http::StatusCode::OK);

    // Second requests from each IP should be 429
    let req = TestRequest::default().peer_addr(ip1).to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::TOO_MANY_REQUESTS);

    let req = TestRequest::default().peer_addr(ip2).to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::TOO_MANY_REQUESTS);
}

#[actix_rt::test]
#[serial]
async fn test_429_body_message_seconds_range() {
    init_test_logger();

    let limiter = Arc::new(LimiterBuilder::new()
        .with_duration(Duration::seconds(2))
        .with_num_requests(1)
        .build());

    let port = start_server(Arc::clone(&limiter)).await;
    let client = Client::new();

    // First OK
    let _ = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    // Second 429 with body
    let resp = client.get(&format!("http://127.0.0.1:{}/", port)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    let body = resp.text().await.unwrap();
    // Body: "Too many requests. Please try again in X seconds."
    let secs = body.split_whitespace().rev().nth(1).unwrap_or("-1");
    let parsed: i64 = secs.parse().unwrap_or(-1);
    assert!(0 <= parsed && parsed <= 2, "body seconds out of range: {} (body: {})", parsed, body);
}

#[actix_rt::test]
#[serial]
async fn test_rate_limiter_with_ipv6_different_ips() {
    init_test_logger();

    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(2))
        .with_num_requests(1)
        .build();

    let service = init_service(
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok)),
    )
    .await;

    // First request from ::1
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 10001))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    // First request from ::2 (some other v6 address)
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 2)), 10002))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

    // Second request from ::1 should be rate-limited
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 10003))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::TOO_MANY_REQUESTS);

    // Second request from ::2 should be rate-limited
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 2)), 10004))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), actix_web::http::StatusCode::TOO_MANY_REQUESTS);
}
