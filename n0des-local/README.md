# n0des-local

`n0des-local` is a minimal implementation of the `iroh-n0des` protocol, which only implements the *Ticket* feature of `iroh-n0des`. It is only intended for tests and local development.

## Use for local development

Start the server with
```
cargo run -p n0des-local
```

This will print a `N0DES_API_SECRET` that you can use in `datum-connect`.


## Use for tests

```rust
let (api_secret, router) = n0des_local::bind_and_start().await?;
// use api_secret
router.shutdown().await?;
```
