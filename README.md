## Datum Tunnels

This repo is broken into 3 components, a CLI, GUI app, and shared-core library that the CLI & GUI draw on.


### Required Tools
* For all three crates: [`rust & cargo`](https://rust-lang.org/tools/install/)
* For UI: [`dioxus`](https://dioxuslabs.com/learn/0.6/getting_started/)
  * specifically, install `dx` with `cargo install dioxus-cli`
  * if you have [`binstall`](https://github.com/cargo-bins/cargo-binstall?tab=readme-ov-file#installation), you can skip compiling `dx` from source by running `cargo binstall dioxus-cli`

### Running CLI commands:
to run without compiling, use `cargo run` in the `cli` directory:

```
cd cli
cargo run -- --help
```

### Running the UI:

to run the UI, make sure you have rust, cargo, and dioxus installed:

```
cd ui
dx serve
```

Note: some functionality (publishing/fetching tickets via n0des) requires `N0DES_API_SECRET`.
If it's not set, the app will run in "local-only" mode and skip n0des integration. See `env.example`.

### Local end-to-end testing (self-hosted n0des)

This repo includes a minimal local n0des-compatible server (`n0des-local`) so you can test publishing/fetching
tickets and full tunnel flows without any external services.

1) Start local n0des (prints an `export N0DES_API_SECRET='...'` line):

```
export RUSTUP_HOME=$HOME/.rustup CARGO_HOME=$HOME/.cargo
export PATH="$HOME/.cargo/bin:$PATH"
RUST_LOG=info cargo run -p n0des-local
```

Note: if you restart `n0des-local`, it will generate a new endpoint identity and print a new
`N0DES_API_SECRET`. You must restart the UI/gateway with the new value (and typically re-create
the proxy) or you’ll see 404s/timeouts.

2) Start a local origin service to tunnel to (example):

```
python3 -m http.server 5173
```

3) Start the UI with `N0DES_API_SECRET` set, create a proxy forwarding to `127.0.0.1:5173`,
and copy the generated **codename** from the listeners list:

```
export N0DES_API_SECRET='(paste from n0des-local)'
cd ui
dx serve
```

4a) Test via the local gateway (routes by `Host` subdomain):

```
export N0DES_API_SECRET='(paste from n0des-local)'
cd cli
export RUST_LOG=info
cargo run -- serve --port 8099
```

Then in another terminal:

```
curl -v --max-time 5 -H "Host: <codename>.localhost" http://127.0.0.1:8099/
```

4b) Or test direct local port-forwarding:

```
export N0DES_API_SECRET='(paste from n0des-local)'
cd cli
cargo run -- connect --addr 127.0.0.1:9000 --codename <codename>
```

Then:

```
curl http://127.0.0.1:9000/
```
