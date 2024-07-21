## rate_limit

A library crate that can be used to add rate limiting middleware to Actix Web application routes.

```rust
use actix_web::{web, App, HttpServer, HttpResponse};
use rate_limit::{LimiterBuilder, RateLimiter};

    HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok))
    })

```

[Actix Web docs regarding App.wrap()](https://docs.rs/actix-web/latest/actix_web/struct.App.html#method.wrap)
