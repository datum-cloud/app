# n0des-local

`n0des-local` is a **minimal, local-only “ticket directory”** for Datum Connect development.

It is **not** a proxy, and it is **not** an iroh relay server.

What it does:
- **Stores** a mapping from **codename → ticket** (e.g. `young-ebony-gem` → `TcpProxyTicket`)
- **Serves** tickets to clients who present the right `N0DES_API_SECRET`
- Supports **publish / get / list / unpublish** operations used by `datum-connect`

What it does *not* do:
- **Does not** carry your HTTP traffic (that’s the gateway + iroh tunnel)
- **Does not** perform NAT traversal/relaying for iroh (iroh handles that; may use public relays)

## Diagram (E2E flow)

```text
                (1) publish ticket (codename -> TcpProxyTicket)
┌──────────────┐  ------------------------------------------->  ┌──────────────┐
│  UI (desktop)│                                                │  n0des-local  │
│  listen_key  │  <-------------------------------------------  │ ticket store  │
└──────┬───────┘     (2) fetch ticket by codename (auth via secret)└──────┬──────┘
       │                                                                  │
       │                                                                  │
       │ (5) iroh QUIC tunnel + CONNECT-style handshake                   │
       │ <--------------------------------------------------------------  │
       │                                                                  │
┌──────▼──────────────────────────────────────────────────────────────────▼──────┐
│                           Gateway (datum-connect serve)                         │
│                           connect_key + connect-only                            │
│     - receives HTTP from browser                                                │
│     - extracts codename from Host header                                        │
│     - fetches ticket from n0des-local                                           │
│     - dials UI endpoint via iroh                                                │
│     - forwards bytes over per-request streams                                   │
└──────┬─────────────────────────────────────────────────────────────────────────┘
       │ (3) HTTP request from browser
       │     Host: <codename>.localhost:8099
       ▼
┌──────────────┐
│   Browser    │
└──────────────┘

Inside the UI machine:

Gateway  --(iroh streams)-->  UI  --(plain TCP)-->  target service (e.g. python http.server)
```

### Key concepts

- **Codename**: human-friendly name derived from tunnel id (`Uuid`) and used as the lookup key.
- **`TcpProxyTicket`**: the “dial + forward” instruction set published to n0des:
  - **where** to dial (iroh `EndpointAddr`: endpoint id + addresses)
  - **what** to forward to (host/port, e.g. `127.0.0.1:8000`)
- **`N0DES_API_SECRET`**: capability token for authenticating to `n0des-local`.
  - Restarting `n0des-local` typically changes the secret; keep UI + gateway in sync.

## How it works (step-by-step)

1) **Start `n0des-local`**
   - It prints a fresh `N0DES_API_SECRET`.

2) **Start the UI with that secret**
   - UI uses `listen_key` and publishes enabled tunnels to n0des-local.

3) **Create a tunnel** in the UI
   - UI writes it to local state (`state.yml`) and publishes a ticket to n0des-local.

4) **Start the gateway (`datum-connect serve`) with that same secret**
   - Gateway uses `connect_key` and dials tunnels; it does **not** publish/announce.

5) **Open in a browser**
   - The browser hits the gateway; the gateway routes by Host header.

## Run locally (known-good E2E)

### Terminal A: start `n0des-local`

```bash
cd /Users/zach/repos/datum/datum-connect
export RUSTUP_HOME=$HOME/.rustup CARGO_HOME=$HOME/.cargo
export PATH="$HOME/.cargo/bin:$PATH"
RUST_LOG=info cargo run -p n0des-local
```

Copy the printed `N0DES_API_SECRET=...`.

### Terminal B: start a local target service

```bash
python3 -m http.server 8000
```

### Terminal C: start the UI (with the secret)

```bash
cd /Users/zach/repos/datum/datum-connect/ui
export N0DES_API_SECRET='(paste from n0des-local)'
dx serve
```

In the UI, create a tunnel pointing to `127.0.0.1:8000`.
Note its codename (e.g. `young-ebony-gem`).

### Terminal D: start the gateway (with the secret)

```bash
cd /Users/zach/repos/datum/datum-connect/cli
export N0DES_API_SECRET='(paste from n0des-local)'
RUST_LOG=info cargo run -- serve --port 8099
```

### Browser

Open:

- `http://<codename>.localhost:8099/`

Example:
- `http://young-ebony-gem.localhost:8099/`

## Enable/disable semantics

In the UI:
- **Enable**: publishes the codename ticket to n0des-local (gateway can resolve/dial it).
- **Disable**: unpublishes the codename ticket (gateway should stop routing immediately).

## Troubleshooting

- **404 from gateway**
  - The codename isn’t published (disabled), or UI/gateway are using a different `N0DES_API_SECRET`.

- **Works once, then stops**
  - If a cached tunnel gets stale, the gateway should redial; restart the gateway if you’re on older builds.

- **n0des-local shows no `ticket get`**
  - The gateway isn’t reaching n0des-local or doesn’t have `N0DES_API_SECRET` set.

