## rate_limit

A library crate that can be used to add rate limiting middleware to a Actix Web server endpoints.

```rust
use actix_web::{web, App, HttpServer, HttpResponse};
use rate_limit::{LimiterBuilder, RateLimiter};

    HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok))
    })

```
