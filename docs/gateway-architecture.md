# Gateway Architecture: Tunnel Design

This document explains the architecture of the datum-connect gateway and how it proxies HTTP requests through iroh/QUIC tunnels to desktop applications.

## Overview

The datum-connect system allows users to expose local services (running on their desktop) to the internet through a secure tunnel. The architecture involves:

1. **Gateway** (cloud) - Receives incoming HTTP requests from the internet
2. **Desktop App** (user's machine) - Runs local services and accepts tunnel connections
3. **iroh/QUIC** - Provides the secure tunnel transport between gateway and desktop

```
Internet                    Cloud                           User's Machine
─────────                   ─────                           ──────────────
                      ┌─────────────┐                     ┌─────────────────┐
  Browser ──HTTP/2──► │   Envoy     │                     │  Desktop App    │
                      │   Proxy     │                     │  (ListenNode)   │
                      └──────┬──────┘                     └────────┬────────┘
                             │                                     │
                      ┌──────▼──────┐     QUIC/iroh       ┌────────▼────────┐
                      │   Gateway   │◄═══════════════════►│  UpstreamProxy  │
                      │ (iroh-gw)   │                     │ (iroh-proxy-    │
                      └─────────────┘                     │     utils)      │
                                                          └────────┬────────┘
                                                                   │
                                                          ┌────────▼────────┐
                                                          │  Local Service  │
                                                          │  (Python, etc)  │
                                                          └─────────────────┘
```

## The Two Architectures

### Original Architecture: CONNECT Tunnels (Deprecated)

The original design used HTTP CONNECT tunnels, which created persistent bidirectional streams tied to TCP sockets.

#### Flow

```
Gateway                                          Desktop
───────                                          ───────

1. Receive H2 request from Envoy
   │
   ▼
2. Check TunnelPool for cached tunnel
   │
   ├─► Pool HIT: Reuse existing tunnel ────────────────────────────┐
   │                                                               │
   └─► Pool MISS: Create new tunnel                                │
       │                                                           │
       ▼                                                           │
3. Send CONNECT request over QUIC stream                           │
   │                                                               │
   │   "CONNECT localhost:5173 HTTP/1.1"                           │
   │   ════════════════════════════════════════════════════►       │
   │                                                               │
   │                                    4. Desktop receives CONNECT│
   │                                       Opens TCP to local svc  │
   │                                       │                       │
   │   ◄════════════════════════════════════════════════════       │
   │   "HTTP/1.1 200 Connection Established"                       │
   │                                                               │
   │                                    5. Desktop runs            │
   │                                       forward_bidi()          │
   │                                       (copies bytes until     │
   │                                        either side closes)    │
   │                                       │                       │
   ▼                                       ▼                       │
6. Send HTTP/1.1 request over tunnel ◄─────────────────────────────┘
   │
   │   "GET /api/users HTTP/1.1"
   │   "Host: localhost:5173"
   │   ════════════════════════════════════════════════════►
   │                                       │
   │                                       ▼
   │                                    7. forward_bidi() copies
   │                                       to TCP socket
   │                                       │
   │                                       ▼
   │                                    8. Python server receives
   │                                       request, sends response
   │                                       │
   │   ◄════════════════════════════════════════════════════
   │   "HTTP/1.1 200 OK"
   │   "Content-Length: 1234"
   │   "..."
   │
   ▼
9. Read response, check keep-alive
   │
   ├─► keep-alive=true: Return tunnel to pool
   │
   └─► keep-alive=false: Close tunnel
```

#### Problems with CONNECT Tunnels

1. **TCP Socket Lifetime Coupling**
   
   The QUIC stream's lifetime was tied to the TCP socket's lifetime:
   ```
   QUIC Stream ←──coupled──► TCP Socket
        │                        │
        │    When TCP closes,    │
        │    stream becomes      │
        │    invalid             │
        ▼                        ▼
   Pooled tunnel is STALE    Python's keep-alive
                             timer expires (5s)
   ```

2. **Stale Tunnel Problem**
   
   When the local service's keep-alive timer expired:
   - Python closes the TCP connection
   - `forward_bidi()` sees EOF, finishes
   - QUIC stream closes
   - Gateway's pooled tunnel is now dead
   - Next request using this tunnel fails

3. **Thundering Herd**
   
   When multiple concurrent requests arrived with an empty pool:
   ```
   Request 1 ──► Pool empty ──► Start creating tunnel
   Request 2 ──► Pool empty ──► Start creating tunnel (simultaneously!)
   Request 3 ──► Pool empty ──► Start creating tunnel (simultaneously!)
   ...
   Result: N concurrent tunnel creations overwhelming iroh
   ```

4. **Observed Symptoms**
   - Intermittent 502 errors ("Failed to read response")
   - 2-8 second latencies for requests
   - Parallel requests performed worse than sequential

---

### New Architecture: Absolute-Form HTTP Requests

The new design sends absolute-form HTTP requests, treating each request as independent and letting the desktop handle TCP connection pooling.

#### Flow

```
Gateway                                          Desktop
───────                                          ───────

1. Receive H2 request from Envoy
   │
   ▼
2. Get/create QUIC connection to endpoint
   (ConnectionManager caches connections,
    NOT streams)
   │
   ▼
3. Open NEW QUIC stream (cheap!)
   │
   ▼
4. Send absolute-form HTTP request
   │
   │   "GET http://localhost:5173/api/users HTTP/1.1"
   │   "Host: localhost:5173"
   │   "Content-Length: 0"
   │   ════════════════════════════════════════════════════►
   │                                       │
   │                                       ▼
   │                                    5. UpstreamProxy parses
   │                                       as Absolute request
   │                                       │
   │                                       ▼
   │                                    6. Uses reqwest::Client
   │                                       (has built-in TCP
   │                                        connection pooling!)
   │                                       │
   │                                       ▼
   │                                    7. reqwest forwards to
   │                                       Python, gets response
   │                                       │
   │   ◄════════════════════════════════════════════════════
   │   "HTTP/1.1 200 OK"
   │   "Content-Length: 1234"
   │   "..."
   │
   ▼
8. Read response
   │
   ▼
9. Close QUIC stream (automatic)
   │
   Done! (TCP connection stays pooled
          in desktop's reqwest::Client)
```

#### Key Differences

| Aspect | CONNECT Tunnels | Absolute-Form HTTP |
|--------|-----------------|-------------------|
| **QUIC stream lifetime** | Long-lived (tied to TCP) | Short-lived (per request) |
| **TCP pooling location** | Gateway (TunnelPool) | Desktop (reqwest::Client) |
| **Gateway complexity** | High (pool, retry, throttling) | Low (just open stream, send) |
| **Desktop complexity** | Low (forward_bidi) | Medium (reqwest handles pooling) |
| **Stale connection risk** | High | None (streams are ephemeral) |
| **Parallel performance** | Poor (thundering herd) | Excellent (streams are cheap) |

#### Why This Works Better

1. **QUIC Streams Are Cheap**
   
   Within an existing QUIC connection, opening a new stream is extremely fast:
   - No handshake required
   - No network round-trip for stream creation
   - Just a local operation

   ```
   QUIC Connection (persistent, reused)
   ════════════════════════════════════════════════════════
   ║                                                      ║
   ║  Stream 1 ──────────────────────► (closes)           ║
   ║  Stream 2 ──────────────────────► (closes)           ║
   ║  Stream 3 ──────────────────────► (closes)           ║
   ║  Stream 4 ──────────────────────► (active)           ║
   ║                                                      ║
   ════════════════════════════════════════════════════════
   ```

2. **TCP Pooling at the Right Place**
   
   The desktop app (via reqwest::Client) is best positioned to manage TCP connections:
   - Knows when local services are available
   - Can detect connection failures immediately
   - Handles retries intelligently

3. **No State to Go Stale**
   
   Each request is independent:
   - Open stream → send request → get response → close stream
   - No cached state that can become invalid
   - No complex retry logic needed

4. **Natural Parallelism**
   
   Concurrent requests each get their own stream:
   ```
   Request 1 ──► Opens Stream 1 ──► Sends HTTP ──► Gets response
   Request 2 ──► Opens Stream 2 ──► Sends HTTP ──► Gets response
   Request 3 ──► Opens Stream 3 ──► Sends HTTP ──► Gets response
   
   (All running concurrently, no contention)
   ```

---

## Implementation Details

### Gateway (lib/src/gateway.rs)

#### ConnectionManager

Caches QUIC connections (not streams) per endpoint:

```rust
struct ConnectionManager {
    endpoint: Endpoint,
    connections: RwLock<HashMap<EndpointId, Connection>>,
}

impl ConnectionManager {
    async fn get_connection(&self, endpoint_id: EndpointId) -> Result<Connection> {
        // Check cache
        if let Some(conn) = self.connections.read().await.get(&endpoint_id) {
            if !conn.close_reason().is_some() {
                return Ok(conn.clone());  // Reuse existing connection
            }
        }
        
        // Create new connection
        let conn = self.endpoint.connect(endpoint_id, ALPN).await?;
        self.connections.write().await.insert(endpoint_id, conn.clone());
        Ok(conn)
    }
}
```

#### Request Handling

Each request opens a fresh stream:

```rust
async fn handle_h2_request(req: Request, conn_manager: Arc<ConnectionManager>) -> Response {
    // 1. Get QUIC connection (cached)
    let conn = conn_manager.get_connection(endpoint_id).await?;
    
    // 2. Open fresh stream (cheap)
    let (send, recv) = conn.open_bi().await?;
    
    // 3. Build absolute-form HTTP request
    let request = build_absolute_http_request(&parts, &destination, &body);
    //  "GET http://localhost:5173/path HTTP/1.1\r\n..."
    
    // 4. Send request, read response
    send.write_all(&request).await?;
    send.finish()?;  // Signal end of request
    
    let response = read_http_response(&mut recv).await?;
    
    // 5. Stream closes automatically when dropped
    Ok(response)
}
```

### Desktop (iroh-proxy-utils)

The `UpstreamProxy` handles absolute-form requests:

```rust
// In iroh-proxy-utils/src/upstream.rs
match req.kind {
    HttpProxyRequestKind::Absolute { method, target } => {
        // Uses reqwest::Client with built-in connection pooling
        let res = http_client
            .request(method, target)
            .headers(req.headers)
            .body(body)
            .send()
            .await?;
        
        // Write response back to QUIC stream
        write_response(&mut send, res).await?;
        send.finish()?;
    }
}
```

---

## Performance Comparison

### Before (CONNECT Tunnels)

```
Sequential requests:
  Request 1: 7925ms  (new tunnel)
  Request 2: 5812ms  (new tunnel - pooled was stale)
  Request 3: 7531ms  (new tunnel)
  Request 4: 7532ms  (new tunnel)
  Request 5:  739ms  (pool HIT!)
  ...

Parallel requests (10 concurrent):
  Request 1: 15014ms (timeout!)
  Request 2: 15015ms (timeout!)
  Request 3: 15015ms (timeout!)
  ...
  (thundering herd overwhelmed the system)
```

### After (Absolute-Form HTTP)

```
Sequential requests:
  Request 1: ~300ms  (new connection + stream)
  Request 2: ~100ms  (cached connection, new stream)
  Request 3: ~100ms  (cached connection, new stream)
  ...

Parallel requests (10 concurrent):
  Request 1:  ~300ms (new connection)
  Request 2:  ~100ms (new stream)
  Request 3:  ~100ms (new stream)
  ...
  (all complete quickly, streams are independent)
```

---

## Summary

The move from CONNECT tunnels to absolute-form HTTP requests:

1. **Eliminates stale tunnel issues** - No long-lived state to go stale
2. **Removes thundering herd** - QUIC streams are cheap to create
3. **Simplifies gateway code** - No TunnelPool, no retry logic
4. **Improves parallel performance** - Each request is independent
5. **Moves TCP pooling to the right place** - Desktop's reqwest handles it

The key insight is that **QUIC streams are cheap** (within an existing connection), while **TCP connections are expensive**. By opening fresh QUIC streams per request and letting the desktop pool TCP connections, we get the best of both worlds.
