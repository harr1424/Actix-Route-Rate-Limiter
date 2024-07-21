## Actix Route Rate Limiter

A library crate that can be used to add rate limiting middleware to Actix Web Application routes.

```rust
use std::sync::Arc;
use actix_web::{web, App, HttpServer, HttpResponse};
use chrono::Duration;
use rate_limit::{LimiterBuilder, RateLimiter};

#[actix_web::main]
pub async fn main() -> std::io::Result<()> {

    // build a limiter
    let limiter = LimiterBuilder::new()
        .with_duration(Duration::seconds(20)) // default value is one second
        .with_num_requests(2) // default value is one request
        .build();


    HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok))
    })
        .bind(("0.0.0.0", 12345))?
        .run()
        .await
}
```

[Actix Web docs regarding App.wrap()](https://docs.rs/actix-web/latest/actix_web/struct.App.html#method.wrap)
