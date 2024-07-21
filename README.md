## rate_limit

A simple [package](https://stackoverflow.com/questions/68250956/what-is-the-exact-difference-between-a-crate-and-a-package) containing a single library crate used to rate-limit endpoints on an Actix-Web server. 

```rust
use actix_web::{web, App, HttpServer, HttpResponse};
use rate_limit::{LimiterBuilder, RateLimiter};

    HttpServer::new(move || {
        App::new()
            .wrap(RateLimiter::new(Arc::clone(&limiter)))
            .route("/", web::get().to(HttpResponse::Ok))
    })

```
