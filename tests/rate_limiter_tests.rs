use actix_web::{
    http::StatusCode,
    test::{call_service, init_service, TestRequest},
    web, App, HttpResponse,
};
use std::sync::Arc;
use rate_limit::{LimiterBuilder, RateLimiter};
use actix_rt::time::sleep;
use std::time::Duration as StdDuration;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use chrono::Duration;

#[actix_rt::test]
async fn test_rate_limiter_allows_two_requests() {
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

    // Simulate two requests from the same IP within 20 seconds
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Third request within 20 seconds should be rate-limited
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    // Check if X-RateLimit-Reset header exists
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());

    // Attempt a fourth request immediately (Expected to fail also)
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    // Check if X-RateLimit-Reset header exists
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());
}

#[actix_rt::test]
async fn test_rate_limiter_allows_after_duration() {
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

    // First request should succeed
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Second request should succeed
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Wait for more than 20 seconds
    sleep(StdDuration::from_secs(21)).await;

    // Third request should also succeed after waiting
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Fourth request after waiting should succeed
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[actix_rt::test]
async fn test_rate_limiter_with_different_ips() {
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
    assert_eq!(resp.status(), StatusCode::OK);

    // First request from IP 127.0.0.2
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Second request from IP 127.0.0.1
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Third request from IP 127.0.0.1 within 20 seconds should be rate-limited
    let req = TestRequest::default()
        .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345))
        .to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("Retry-After").unwrap(), "20");
    assert_eq!(resp.headers().get("X-RateLimit-Limit").unwrap(), "2");
    assert_eq!(resp.headers().get("X-RateLimit-Remaining").unwrap(), "0");
    // Check if X-RateLimit-Reset header exists
    assert!(resp.headers().get("X-RateLimit-Reset").is_some());
}

#[actix_rt::test]
async fn test_rate_limiter_handles_missing_ip() {
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

    // Simulate a request without an IP address
    let req = TestRequest::default().to_request();
    let resp = call_service(&service, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
}
