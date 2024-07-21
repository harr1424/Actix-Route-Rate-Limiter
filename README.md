## rate_limit

A library crate that can be used to add rate limiting middleware to Actix Web application routes.

```rust
use actix_web::{web, App, HttpServer, HttpResponse};
use rate_limit::{LimiterBuilder, RateLimiter};

#[actix_web::main]
pub async fn run() -> std::io::Result<()> {

    // build a limiter
    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20)) // default one second
        .with_num_requests(2) // default one request
        .build();


    HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok)),

    })
        .bind(("0.0.0.0", 12345))?
        .run()
        .await
}

```

[Actix Web docs regarding App.wrap()](https://docs.rs/actix-web/latest/actix_web/struct.App.html#method.wrap)
