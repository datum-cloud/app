## Datum Tunnels

This repo is broken into 3 components, a CLI, GUI app, and shared-core library that the CLI & GUI draw on.

### Download the app
[![Download for macOS](https://img.shields.io/badge/Download-macOS-000000?logo=apple&logoColor=white)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum.dmg)
[![Download for Windows](https://img.shields.io/badge/Download-Windows-0078D6?logo=windows&logoColor=white)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum-setup.exe)
[![Download for Linux](https://img.shields.io/badge/Download-Linux-FCC624?logo=linux&logoColor=black)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum.AppImage)

If a download fails, use the latest release page: https://github.com/datum-cloud/datum-connect/releases/latest


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

### Local forward-proxy demo (no GUI)
This exercises the CONNECT-based gateway flow that Envoy will use in staging/prod.

#### 1) Start a local DNS dev server (out-of-band)
Use a non-`.local` origin (e.g. `datumconnect.test`):

```
cargo run -p datum-connect -- dns-dev serve \
  --origin datumconnect.test \
  --bind 127.0.0.1:53535 \
  --data ./dns-dev.yml
```

#### 2) Start the listen node (connector side)
This prints the endpoint id and the iroh UDP bound sockets you must publish:

```
cargo run -p datum-connect -- serve
```

Copy the printed `dns-dev upsert` example, but run it via `cargo run -p datum-connect -- ...`
and make sure the origin matches `datumconnect.test`. Quote IPv6 addresses like `"[::]:1234"`.

#### 3) Verify TXT resolution
The `serve` command prints the z-base-32 ID and the full DNS name. Query it with:

```
dig +norecurse @127.0.0.1 -p 53535 TXT _iroh.<z32>.datumconnect.test
```

#### 4) Start the gateway in forward mode

```
cargo run -p datum-connect -- gateway \
  --port 8080 \
  --metrics-addr 127.0.0.1 \
  --metrics-port 9090 \
  --mode forward \
  --discovery dns \
  --dns-origin datumconnect.test \
  --dns-resolver 127.0.0.1:53535

Discovery modes:
- `default`: iroh defaults (n0 preset).
- `dns`: only the provided DNS origin/resolver.
- `hybrid`: default + custom DNS.
- metrics endpoint: `GET http://127.0.0.1:9090/metrics` (when `--metrics-addr` or `--metrics-port` is set)
```

#### 5) Send a CONNECT request
If your target TCP service is on `127.0.0.1:5173`:

```
curl --proxytunnel -x 127.0.0.1:8080 \
  --proxy-header "x-iroh-endpoint-id: REPLACE_WITH_ENDPOINT_ID" \
  "http://127.0.0.1:5173"
```

### GUI demo (browser tunnel)
This mirrors the same flow, but uses the GUI to create the proxy entry.

If you want a one-shot experience, run:

```
./scripts/try-ui-demo.sh
```

It starts dns-dev, an HTTPS origin, the gateway, and the GUI, and waits for you to
create a TCP proxy in the UI before visiting `https://localhost:5173` in the browser.

#### 1) Start `dns-dev`
```
cargo run -p datum-connect -- dns-dev serve \
  --origin datumconnect.test \
  --bind 127.0.0.1:53535 \
  --data ./dns-dev.yml
```

#### 2) Start a local HTTPS origin (so the browser uses CONNECT)
```
openssl req -x509 -nodes -newkey rsa:2048 -days 1 \
  -keyout /tmp/iroh-dev.key -out /tmp/iroh-dev.crt \
  -subj "/CN=localhost"
openssl s_server -accept 5173 -cert /tmp/iroh-dev.crt -key /tmp/iroh-dev.key -www
```

#### 3) Run the GUI (share the repo with CLI)
```
export DATUM_CONNECT_REPO=$(pwd)/.datum-connect-dev
cd ui
dx serve --platform desktop
```

#### 4) Create a proxy in the GUI
Add a TCP proxy for `127.0.0.1:5173`.

#### 5) Start the listen node (uses the same repo)
```
cd ..
export DATUM_CONNECT_REPO=$(pwd)/.datum-connect-dev
cargo run -p datum-connect -- serve
```
Copy the printed `dns-dev upsert` example, but change the origin to `datumconnect.test`
and run it via `cargo run -p datum-connect -- ...` (quote IPv6 addresses).

#### 6) Start the gateway in forward mode
```
export DATUM_CONNECT_REPO=$(pwd)/.datum-connect-dev
cargo run -p datum-connect -- gateway \
  --port 8080 \
  --metrics-addr 127.0.0.1 \
  --metrics-port 9090 \
  --mode forward \
  --discovery dns \
  --dns-origin datumconnect.test \
  --dns-resolver 127.0.0.1:53535
```

#### 7) Start a local entrypoint that always tunnels through the gateway
This avoids any browser proxy configuration. It listens on `127.0.0.1:8888` and
uses CONNECT under the hood to reach the target:
```
cargo run -p datum-connect -- tunnel-dev \
  --gateway 127.0.0.1:8080 \
  --node-id REPLACE_WITH_ENDPOINT_ID \
  --target-host 127.0.0.1 \
  --target-port 5173
```
Now visit:
```
https://localhost:8888
```
You should see the `openssl s_server` status page (cipher list + handshake info).
That output is expected and means the CONNECT request tunneled through the gateway
to the local origin.

### Running the UI:

to run the UI, make sure you have rust, cargo, and dioxus installed:

```
cd ui
dx serve
```
