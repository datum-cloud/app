use std::{collections::HashMap, io, net::SocketAddr, str::FromStr, sync::Arc, time::Instant};

use askama::Template;
use http_body_util::{BodyExt, Full};
use hyper::{
    Request, StatusCode, body::Bytes, http::{self, HeaderMap, HeaderName, HeaderValue, Method},
    server::conn::http2, service::service_fn,
};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use iroh::{Endpoint, EndpointId, SecretKey};
use iroh_proxy_utils::{
    ALPN as IROH_HTTP_PROXY_ALPN, Authority, HttpOriginRequest, HttpProxyRequest,
    HttpProxyRequestKind, HttpResponse,
    downstream::{
        DownstreamProxy, EndpointAuthority, ExtractError, ForwardProxyResolver, HttpProxyOpts,
        ProxyMode, ReverseProxyResolver, WriteErrorResponse,
    },
};
use n0_error::Result;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::RwLock,
};
use tracing::{Instrument, debug, info, warn};

use crate::{
    FetchTicketError, TicketClient, build_endpoint, build_n0des_client, n0des_api_secret_from_env,
};

pub async fn bind_and_serve(
    secret_key: SecretKey,
    config: crate::config::GatewayConfig,
    tcp_bind_addr: SocketAddr,
) -> Result<()> {
    let listener = TcpListener::bind(tcp_bind_addr).await?;
    let endpoint = build_endpoint(secret_key, &config.common).await?;
    let n0des = match config.gateway_mode {
        crate::config::GatewayMode::Reverse => {
            let n0des_api_secret = n0des_api_secret_from_env()?;
            Some(build_n0des_client(&endpoint, n0des_api_secret).await?)
        }
        crate::config::GatewayMode::Forward => None,
    };
    serve(endpoint, n0des, listener, config).await
}

pub async fn serve(
    endpoint: Endpoint,
    n0des: Option<Arc<iroh_n0des::Client>>,
    listener: TcpListener,
    config: crate::config::GatewayConfig,
) -> Result<()> {
    let tcp_bind_addr = listener.local_addr()?;
    info!(
        ?tcp_bind_addr,
        endpoint_id = %endpoint.id().fmt_short(),
        gateway_mode = ?config.gateway_mode,
        "TCP proxy gateway started"
    );

    // Clone endpoint for direct stream access (absolute-form HTTP requests)
    let endpoint_for_streams = endpoint.clone();
    let proxy = DownstreamProxy::new(endpoint, Default::default());
    let mode = match config.gateway_mode {
        crate::config::GatewayMode::Reverse => {
            let n0des = n0des.ok_or_else(|| {
                n0_error::anyerr!("n0des client is required for reverse gateway mode")
            })?;
            let tickets = TicketClient::new(n0des);
            let resolver = Resolver { tickets };
            let opts = HttpProxyOpts::default()
                // Right now the gateway functions as a reverse proxy, i.e. incoming requests are regular origin-form HTTP
                // requests, and we resolve the destination from the host header's subdomain.
                .reverse(resolver)
                .error_response_writer(ErrorResponseWriter);
            ProxyMode::Http(opts)
        }
        crate::config::GatewayMode::Forward => {
            let resolver = ForwardResolver;
            let header_resolver = HeaderResolver;
            let opts = HttpProxyOpts::default()
                // Forward proxy mode accepts CONNECT authority-form requests.
                .forward(resolver)
                // Also allow origin-form requests that carry tunnel headers.
                .reverse(header_resolver)
                .error_response_writer(ErrorResponseWriter);
            ProxyMode::Http(opts)
        }
    };

    match config.gateway_mode {
        crate::config::GatewayMode::Forward => {
            let conn_manager = Arc::new(ConnectionManager::new(endpoint_for_streams));
            forward_tcp_listener_with_h2c(proxy, listener, mode, conn_manager).await
        }
        crate::config::GatewayMode::Reverse => proxy.forward_tcp_listener(listener, mode).await,
    }
}

#[derive(Clone)]
struct Resolver {
    tickets: TicketClient,
}

/// When operating in reverse-proxy mode we accept origin-form http requests,
/// and need to resolve their full destination.
///
/// This is the currently-deployed version, which uses the subdomain.
impl ReverseProxyResolver for Resolver {
    async fn destination<'a>(
        &'a self,
        req: &'a HttpOriginRequest,
    ) -> Result<EndpointAuthority, ExtractError> {
        let host = req.host().ok_or(ExtractError::BadRequest)?;
        let codename = extract_subdomain(host).ok_or(ExtractError::NotFound)?;

        debug!(%codename, "extracted codename, resolving ticket...");
        let ticket = self.tickets.get(codename).await.map_err(|err| {
            debug!(%codename, "failed to resolve ticket: {err:#}");
            match err {
                FetchTicketError::NotFound => ExtractError::NotFound,
                FetchTicketError::FailedToFetch(_) => ExtractError::ServiceUnavailable,
            }
        })?;
        debug!(?ticket, "resolved ticket");
        Ok(EndpointAuthority {
            endpoint_id: ticket.endpoint,
            authority: ticket.data.data.into(),
        })
    }
}

const HEADER_NODE_ID: &str = "x-iroh-endpoint-id";
const HEADER_TARGET_HOST: &str = "x-datum-target-host";
const HEADER_TARGET_PORT: &str = "x-datum-target-port";
const HEADER_HOST: &str = "host";

/// When operating in forward-proxy mode we accept CONNECT requests and resolve the target
/// endpoint ID from headers injected by Envoy.
struct ForwardResolver;

impl ForwardProxyResolver for ForwardResolver {
    async fn destination<'a>(
        &'a self,
        req: &'a HttpProxyRequest,
    ) -> Result<EndpointId, ExtractError> {
        if !matches!(req.kind, HttpProxyRequestKind::Tunnel { .. }) {
            return Err(ExtractError::BadRequest);
        }
        let node_id = header_value(req, HEADER_NODE_ID).ok_or(ExtractError::BadRequest)?;
        EndpointId::from_str(node_id).map_err(|_| ExtractError::BadRequest)
    }
}

struct HeaderResolver;

impl ReverseProxyResolver for HeaderResolver {
    async fn destination<'a>(
        &'a self,
        req: &'a HttpOriginRequest,
    ) -> Result<EndpointAuthority, ExtractError> {
        let endpoint_id = endpoint_id_from_headers(&req.headers)?;
        let host = header_value_map(&req.headers, HEADER_TARGET_HOST)
            .ok_or(ExtractError::BadRequest)?;
        let port = header_value_map(&req.headers, HEADER_TARGET_PORT)
            .ok_or(ExtractError::BadRequest)?;
        let port = port.parse::<u16>().map_err(|_| ExtractError::BadRequest)?;
        Ok(EndpointAuthority {
            endpoint_id,
            authority: Authority {
                host: host.to_string(),
                port,
            },
        })
    }
}

fn endpoint_id_from_headers(
    headers: &HeaderMap<HeaderValue>,
) -> Result<EndpointId, ExtractError> {
    let node_id = header_value_map(headers, HEADER_NODE_ID).ok_or(ExtractError::BadRequest)?;
    EndpointId::from_str(node_id).map_err(|_| ExtractError::BadRequest)
}

fn header_value<'a>(req: &'a HttpProxyRequest, name: &str) -> Option<&'a str> {
    header_value_map(&req.headers, name)
}

fn header_value_map<'a>(headers: &'a HeaderMap<HeaderValue>, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const H2_PREFACE_LEN: usize = 24;
const HTTP1_HEADER_MAX_LEN: usize = 64 * 1024;

async fn forward_tcp_listener_with_h2c(
    proxy: DownstreamProxy,
    listener: TcpListener,
    mode: ProxyMode,
    conn_manager: Arc<ConnectionManager>,
) -> Result<()> {
    let mut id: u64 = 0;
    let active_connections = Arc::new(std::sync::atomic::AtomicU64::new(0));
    
    loop {
        let (stream, client_addr) = listener.accept().await?;
        let proxy = proxy.clone();
        let mode = mode.clone();
        let conn_manager = conn_manager.clone();
        let active_connections = active_connections.clone();
        
        let active = active_connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        info!(%client_addr, connection_id = id, active_connections = active, "new TCP connection accepted");
        
        tokio::spawn(
            async move {
                let is_h2 = match is_h2c_preface(&stream).await {
                    Ok(is_h2) => is_h2,
                    Err(err) => {
                        warn!(%client_addr, "failed to peek connection: {err:#}");
                        false
                    }
                };
                
                let result = if is_h2 {
                    debug!(%client_addr, "detected h2c preface");
                    forward_h2c_connection(stream, conn_manager).await
                } else {
                    debug!(%client_addr, "detected HTTP/1.x");
                    proxy.forward_tcp_stream(stream, &mode).await
                };
                
                if let Err(err) = result {
                    warn!(%client_addr, "connection failed: {err:#}");
                }
                
                let remaining = active_connections.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
                debug!(%client_addr, active_connections = remaining, "connection closed");
            }
            .instrument(tracing::error_span!("tcp-accept", id)),
        );
        id += 1;
    }
}

async fn is_h2c_preface(stream: &tokio::net::TcpStream) -> io::Result<bool> {
    let mut buf = [0u8; H2_PREFACE_LEN];
    let n = stream.peek(&mut buf).await?;
    if n == 0 {
        return Ok(false);
    }
    if n < H2_PREFACE_LEN {
        Ok(H2_PREFACE.starts_with(&buf[..n]))
    } else {
        Ok(&buf == H2_PREFACE)
    }
}

async fn forward_h2c_connection(
    stream: tokio::net::TcpStream,
    conn_manager: Arc<ConnectionManager>,
) -> Result<()> {
    let peer_addr = stream.peer_addr().ok();
    info!(?peer_addr, "new h2c connection accepted");
    
    let io = TokioIo::new(stream);
    let request_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let request_count_clone = request_count.clone();
    
    let service = service_fn(move |req| {
        let count = request_count_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug!(?peer_addr, request_num = count, "h2 stream opened");
        handle_h2_request(req, conn_manager.clone())
    });
    
    let result = http2::Builder::new(TokioExecutor::new())
        .timer(TokioTimer::new())
        .keep_alive_interval(Some(std::time::Duration::from_secs(10)))
        .keep_alive_timeout(std::time::Duration::from_secs(30))
        .serve_connection(io, service)
        .await;
    
    let total_requests = request_count.load(std::sync::atomic::Ordering::Relaxed);
    match &result {
        Ok(()) => info!(?peer_addr, total_requests, "h2c connection closed normally"),
        Err(err) => warn!(?peer_addr, total_requests, "h2c connection error: {err:#}"),
    }
    
    result.map_err(|err| n0_error::anyerr!(err))?;
    Ok(())
}

async fn handle_h2_request(
    req: Request<hyper::body::Incoming>,
    conn_manager: Arc<ConnectionManager>,
) -> Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
    let (parts, body) = req.into_parts();
    let request_id = parts
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| parts.uri.path())
        .to_string();
    let start = Instant::now();
    let origin_req = HttpOriginRequest {
        path: path.clone(),
        method: parts.method.clone(),
        headers: parts.headers.clone(),
    };

    // Log incoming request details for SNI/hostname verification
    let incoming_host = parts.headers.get("host")
        .or_else(|| parts.headers.get(":authority"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>");
    let incoming_endpoint_id = parts.headers.get("x-iroh-endpoint-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>");
    let incoming_target_host = parts.headers.get("x-datum-target-host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>");
    let incoming_target_port = parts.headers.get("x-datum-target-port")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>");
    
    info!(
        %request_id,
        %path,
        host = %incoming_host,
        endpoint_id = %incoming_endpoint_id,
        target_host = %incoming_target_host,
        target_port = %incoming_target_port,
        "h2 request received - verifying SNI/hostname transfer"
    );

    let destination = match HeaderResolver.destination(&origin_req).await {
        Ok(destination) => destination,
        Err(err) => {
            let status = err.response_status();
            warn!(
                %request_id,
                %path,
                status = %status,
                "h2 request rejected before tunnel"
            );
            return Ok(h2_error_response(status, "Invalid tunnel headers"));
        }
    };

    info!(
        %request_id,
        %path,
        endpoint_id = %destination.endpoint_id.fmt_short(),
        target_host = %destination.authority.host,
        target_port = destination.authority.port,
        "h2 request resolved destination - routing to desktop"
    );

    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            warn!(%request_id, %path, "failed to read h2 request body");
            return Ok(h2_error_response(
                StatusCode::BAD_REQUEST,
                "Failed to read request body",
            ));
        }
    };

    // Build absolute-form HTTP request (not CONNECT tunnel)
    // Desktop's reqwest::Client will handle TCP connection pooling
    let request_bytes =
        build_absolute_http_request(&parts, &destination, &body_bytes, &origin_req.headers);

    // Open a fresh QUIC stream for each request (streams are cheap, QUIC connection is pooled)
    let conn_start = Instant::now();
    let conn = match conn_manager.get_connection(destination.endpoint_id).await {
        Ok(conn) => conn,
        Err(err) => {
            warn!(%request_id, %path, "failed to get connection: {err:#}");
            return Ok(h2_error_response(
                StatusCode::BAD_GATEWAY,
                "Failed to connect to upstream",
            ));
        }
    };
    let conn_ms = conn_start.elapsed().as_millis();

    let stream_start = Instant::now();
    let (mut send, recv) = match conn.open_bi().await {
        Ok(streams) => streams,
        Err(err) => {
            warn!(%request_id, %path, "failed to open stream: {err:#}");
            // Connection might be stale, remove it and retry once
            conn_manager.remove_connection(destination.endpoint_id).await;
            let conn = match conn_manager.get_connection(destination.endpoint_id).await {
                Ok(conn) => conn,
                Err(err) => {
                    warn!(%request_id, %path, "retry connection failed: {err:#}");
        return Ok(h2_error_response(
            StatusCode::BAD_GATEWAY,
                        "Failed to connect to upstream",
                    ));
                }
            };
            match conn.open_bi().await {
                Ok(streams) => streams,
                Err(err) => {
                    warn!(%request_id, %path, "retry open stream failed: {err:#}");
                    return Ok(h2_error_response(
                        StatusCode::BAD_GATEWAY,
                        "Failed to open stream",
                    ));
                }
            }
        }
    };
    let stream_ms = stream_start.elapsed().as_millis();

    // Send the HTTP request
    let send_start = Instant::now();
    if let Err(err) = send.write_all(&request_bytes).await {
        warn!(%request_id, %path, "failed to send request: {err:#}");
        return Ok(h2_error_response(
            StatusCode::BAD_GATEWAY,
            "Failed to send request",
        ));
    }
    if !body_bytes.is_empty() {
        if let Err(err) = send.write_all(&body_bytes).await {
            warn!(%request_id, %path, "failed to send body: {err:#}");
            return Ok(h2_error_response(
                StatusCode::BAD_GATEWAY,
                "Failed to send request body",
            ));
        }
    }
    // Signal end of request
    if let Err(err) = send.finish() {
        warn!(%request_id, %path, "failed to finish send: {err:#}");
    }
    let send_ms = send_start.elapsed().as_millis();

    // Read the HTTP response
    let recv_start = Instant::now();
    let mut stream_reader = StreamReader { recv, read_buf: Vec::new() };
    let (status, headers, response_body) = match read_http1_response_from_stream(&mut stream_reader, &origin_req.method).await {
        Ok(result) => result,
            Err(err) => {
            warn!(%request_id, %path, "failed to read response: {err:#}");
                return Ok(h2_error_response(
                    StatusCode::BAD_GATEWAY,
                    "Failed to read response",
                ));
            }
        };
    let recv_ms = recv_start.elapsed().as_millis();

    info!(
        %request_id,
        %path,
        status = %status,
        conn_ms,
        stream_ms,
        send_ms,
        recv_ms,
        total_ms = start.elapsed().as_millis(),
        "request complete"
    );

    Ok(build_h2_response(status, headers, response_body))
}

/// Build an absolute-form HTTP request for the desktop's UpstreamProxy.
/// Format: "GET http://host:port/path HTTP/1.1\r\n..."
/// The desktop's reqwest::Client will handle TCP connection pooling.
fn build_absolute_http_request(
    parts: &http::request::Parts,
    destination: &EndpointAuthority,
    body: &[u8],
    headers: &HeaderMap,
) -> Vec<u8> {
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| parts.uri.path());
    
    // Build absolute-form URL: http://host:port/path
    let absolute_url = format!(
        "http://{}:{}{}",
        destination.authority.host, destination.authority.port, path
    );
    
    let mut buffer = String::new();
    buffer.push_str(&format!("{} {} HTTP/1.1\r\n", parts.method, absolute_url));

    // Add headers
    for (name, value) in headers.iter() {
        if should_skip_h2_header(name) {
            continue;
        }
        let Ok(value) = value.to_str() else { continue };
        buffer.push_str(name.as_str());
        buffer.push_str(": ");
        buffer.push_str(value);
        buffer.push_str("\r\n");
    }
    
    // Always add Host header for absolute-form requests
        buffer.push_str(&format!(
            "Host: {}:{}\r\n",
            destination.authority.host, destination.authority.port
        ));
    buffer.push_str(&format!("Content-Length: {}\r\n", body.len()));
    buffer.push_str("\r\n");
    buffer.into_bytes()
}

fn should_skip_h2_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "proxy-connection"
            | "upgrade"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "content-length"
            | "x-iroh-endpoint-id"
            | "x-datum-target-host"
            | "x-datum-target-port"
    )
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

fn build_h2_response(
    status: StatusCode,
    mut headers: HeaderMap,
    body: Vec<u8>,
) -> hyper::Response<Full<Bytes>> {
    strip_h1_only_headers(&mut headers);
    headers.insert(
        http::header::CONTENT_LENGTH,
        HeaderValue::from_str(&body.len().to_string()).unwrap(),
    );
    let mut builder = hyper::Response::builder().status(status);
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    builder
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| h2_error_response(StatusCode::BAD_GATEWAY, "Invalid response"))
}

fn strip_h1_only_headers(headers: &mut HeaderMap) {
    for name in [
        "connection",
        "proxy-connection",
        "upgrade",
        "keep-alive",
        "transfer-encoding",
        "te",
    ] {
        headers.remove(name);
    }
}

fn h2_error_response(status: StatusCode, message: &str) -> hyper::Response<Full<Bytes>> {
    let body = message.as_bytes().to_vec();
    hyper::Response::builder()
        .status(status)
        .header(
            http::header::CONTENT_LENGTH,
            HeaderValue::from_str(&body.len().to_string()).unwrap(),
        )
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

/// Manages QUIC connections to desktop endpoints.
/// Connections are cached and reused; streams within connections are per-request.
struct ConnectionManager {
    endpoint: Endpoint,
    /// Cache of QUIC connections per endpoint ID
    connections: RwLock<HashMap<EndpointId, iroh::endpoint::Connection>>,
}

impl ConnectionManager {
    fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a QUIC connection to the specified endpoint.
    /// Connections are cached and reused for multiple streams.
    async fn get_connection(&self, endpoint_id: EndpointId) -> Result<iroh::endpoint::Connection> {
        // Try to get existing connection
        if let Some(conn) = self.connections.read().await.get(&endpoint_id) {
            if !conn.close_reason().is_some() {
                debug!(endpoint_id = %endpoint_id.fmt_short(), "reusing cached QUIC connection");
                return Ok(conn.clone());
            }
        }

        // Create new connection
        info!(endpoint_id = %endpoint_id.fmt_short(), "creating new QUIC connection");
        let conn = self
            .endpoint
            .connect(endpoint_id, IROH_HTTP_PROXY_ALPN)
            .await?;
        
        // Cache it
        self.connections.write().await.insert(endpoint_id, conn.clone());
        
        Ok(conn)
    }

    /// Remove a cached connection (e.g., when it's known to be stale)
    async fn remove_connection(&self, endpoint_id: EndpointId) {
        self.connections.write().await.remove(&endpoint_id);
    }
}

/// Wrapper for reading HTTP responses from a QUIC RecvStream
struct StreamReader {
    recv: iroh::endpoint::RecvStream,
    read_buf: Vec<u8>,
}

/// Read HTTP/1.1 response from a StreamReader (for absolute-form requests)
async fn read_http1_response_from_stream(
    reader: &mut StreamReader,
    method: &Method,
) -> Result<(StatusCode, HeaderMap, Vec<u8>)> {
    let header_end = read_until_header_end_stream(reader).await?;
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut res = httparse::Response::new(&mut headers);
    let header_bytes = reader.read_buf[..header_end].to_vec();
    match res.parse(&header_bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => {
            return Err(n0_error::anyerr!(
                "Incomplete HTTP response headers"
            ));
        }
        Err(err) => return Err(n0_error::anyerr!("{err}")),
    }

    reader.read_buf.drain(..header_end);

    let status = res
        .code
        .ok_or_else(|| n0_error::anyerr!("Missing response status code"))?;
    let status = StatusCode::from_u16(status)
        .map_err(|err| n0_error::anyerr!("{err}"))?;
    let headers = HeaderMap::from_iter(res.headers.iter().flat_map(|h| {
        let value = HeaderValue::from_bytes(h.value).ok()?;
        let name = HeaderName::from_bytes(h.name.as_bytes()).ok()?;
        Some((name, value))
    }));

    if matches!(
        status,
        StatusCode::CONTINUE
            | StatusCode::SWITCHING_PROTOCOLS
            | StatusCode::PROCESSING
            | StatusCode::NO_CONTENT
            | StatusCode::NOT_MODIFIED
    ) || *method == Method::HEAD
    {
        return Ok((status, headers, Vec::new()));
    }

    if let Some(length) = content_length(&headers) {
        let body = read_exact_from_stream(reader, length).await?;
        return Ok((status, headers, body));
    }

    if is_chunked(&headers) {
        let body = read_chunked_from_stream(reader).await?;
        return Ok((status, headers, body));
        }

    // No Content-Length and not chunked - read until EOF
    let mut body = reader.read_buf.split_off(0);
    read_to_end_stream(reader, &mut body).await?;
    Ok((status, headers, body))
}

async fn read_until_header_end_stream(reader: &mut StreamReader) -> Result<usize> {
    loop {
        if let Some(pos) = find_header_end(&reader.read_buf) {
            return Ok(pos);
        }
        if reader.read_buf.len() >= HTTP1_HEADER_MAX_LEN {
            return Err(n0_error::anyerr!(
                "HTTP response headers too large"
            ));
        }
        // Use read_chunk which is the native quinn API
        match reader.recv.read_chunk(8192, true).await {
            Ok(Some(chunk)) => {
                reader.read_buf.extend_from_slice(&chunk.bytes);
            }
            Ok(None) => {
            return Err(n0_error::anyerr!("Unexpected EOF while reading headers"));
        }
            Err(err) => {
                return Err(n0_error::anyerr!("Read error: {err}"));
            }
        }
    }
}

async fn read_exact_from_stream(reader: &mut StreamReader, len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; len];
    let mut offset = 0;
    if !reader.read_buf.is_empty() {
        let take = len.min(reader.read_buf.len());
        out[..take].copy_from_slice(&reader.read_buf[..take]);
        reader.read_buf.drain(..take);
        offset = take;
    }
    while offset < len {
        match reader.recv.read_chunk(len - offset, true).await {
            Ok(Some(chunk)) => {
                let take = (len - offset).min(chunk.bytes.len());
                out[offset..offset + take].copy_from_slice(&chunk.bytes[..take]);
                offset += take;
                // If chunk has more data than we need, buffer the rest
                if chunk.bytes.len() > take {
                    reader.read_buf.extend_from_slice(&chunk.bytes[take..]);
                }
            }
            Ok(None) => {
                return Err(n0_error::anyerr!("Unexpected EOF while reading body"));
            }
            Err(err) => {
                return Err(n0_error::anyerr!("Read error: {err}"));
            }
        }
    }
    Ok(out)
}

async fn read_to_end_stream(reader: &mut StreamReader, buf: &mut Vec<u8>) -> Result<()> {
    loop {
        match reader.recv.read_chunk(8192, true).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk.bytes);
            }
            Ok(None) => break,
            Err(err) => {
                return Err(n0_error::anyerr!("Read error: {err}"));
            }
        }
    }
    Ok(())
}

fn content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
}

fn is_chunked(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::TRANSFER_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
}

async fn read_chunked_from_stream(reader: &mut StreamReader) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line = read_line_from_stream(reader).await?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let size_str = line.split(';').next().unwrap_or(line);
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| n0_error::anyerr!("Invalid chunk size"))?;
        if size == 0 {
            loop {
                let trailer = read_line_from_stream(reader).await?;
                if trailer.trim().is_empty() {
                    return Ok(body);
                }
            }
        }
        let chunk = read_exact_from_stream(reader, size).await?;
        body.extend_from_slice(&chunk);
        let _ = read_exact_from_stream(reader, 2).await?;
    }
}

async fn read_line_from_stream(reader: &mut StreamReader) -> Result<String> {
    loop {
        if let Some(pos) = reader.read_buf.windows(2).position(|w| w == b"\r\n") {
            let line = reader.read_buf[..pos].to_vec();
            reader.read_buf.drain(..pos + 2);
            return Ok(String::from_utf8_lossy(&line).to_string());
        }
        if reader.read_buf.len() >= HTTP1_HEADER_MAX_LEN {
            return Err(n0_error::anyerr!("Line too long while parsing chunked body"));
        }
        match reader.recv.read_chunk(8192, true).await {
            Ok(Some(chunk)) => {
                reader.read_buf.extend_from_slice(&chunk.bytes);
            }
            Ok(None) => {
            return Err(n0_error::anyerr!("Unexpected EOF while reading chunked body"));
        }
            Err(err) => {
                return Err(n0_error::anyerr!("Read error: {err}"));
            }
        }
    }
}

pub(super) fn extract_subdomain(host: &str) -> Option<&str> {
    let host = host
        .rsplit_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(host);
    if host.parse::<std::net::IpAddr>().is_ok() {
        None
    } else {
        host.split_once(".").map(|(first, _rest)| first)
    }
}

#[derive(Template)]
#[template(path = "gateway_error.html")]
struct GatewayErrorTemplate<'a> {
    title: &'a str,
    body: &'a str,
}

struct ErrorResponseWriter;

impl WriteErrorResponse for ErrorResponseWriter {
    async fn write_error_response<'a>(
        &'a self,
        res: &'a HttpResponse,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin),
    ) -> io::Result<()> {
        let title = format!("{} {}", res.status.as_u16(), res.reason());
        let body = match res.status {
            StatusCode::BAD_REQUEST => {
                "The request could not be understood by the gateway. Please try again."
            }
            StatusCode::UNAUTHORIZED => {
                "You are not logged in or your session has expired. Please sign in and try again."
            }
            StatusCode::FORBIDDEN => "Access to this resource is not allowed through the gateway.",
            StatusCode::NOT_FOUND => "The requested page could not be found through the gateway.",
            StatusCode::INTERNAL_SERVER_ERROR => {
                "The gateway encountered an internal error. Please try again later."
            }
            StatusCode::BAD_GATEWAY => {
                "The gateway could not get a valid response from the upstream service."
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                "The service is temporarily unavailable. Please try again shortly."
            }
            StatusCode::GATEWAY_TIMEOUT => "The upstream service took too long to respond.",
            _ => "The service experienced an unexpected error.",
        };
        let html = GatewayErrorTemplate {
            body: &body,
            title: &title,
        }
        .render()
        .unwrap_or(title);
        writer.write_all(res.status_line().as_bytes()).await?;
        let headers = format!(
            "Content-Type: text/html\r\nContent-Length: {}\r\n\r\n",
            html.len()
        );
        writer.write_all(headers.as_bytes()).await?;
        writer.write_all(html.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue, Method};
    use hyper::http;
    use iroh_proxy_utils::{Authority, downstream::EndpointAuthority};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::{
        H2_PREFACE, build_absolute_http_request, extract_subdomain, find_header_end,
        is_h2c_preface, content_length, is_chunked,
    };

    // ==================== Subdomain extraction tests ====================

    #[test]
    fn extract_subdomain_from_host_with_port() {
        assert_eq!(extract_subdomain("alpha.example.test:8080"), Some("alpha"));
    }

    #[test]
    fn extract_subdomain_from_host_without_port() {
        assert_eq!(extract_subdomain("beta.example.test"), Some("beta"));
    }

    #[test]
    fn extract_subdomain_rejects_ip() {
        assert_eq!(extract_subdomain("127.0.0.1:8080"), None);
    }

    #[test]
    fn extract_subdomain_returns_none_without_dot() {
        assert_eq!(extract_subdomain("localhost:8080"), None);
    }

    #[test]
    fn extract_subdomain_rejects_ipv6_literal() {
        assert_eq!(extract_subdomain("[::1]:8080"), None);
    }

    // ==================== H2C preface detection tests ====================

    #[tokio::test]
    async fn detects_h2c_preface() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            is_h2c_preface(&stream).await.unwrap()
        });

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        client.write_all(H2_PREFACE).await.unwrap();
        client.flush().await.unwrap();

        assert!(server.await.unwrap());
    }

    #[tokio::test]
    async fn detects_non_h2c() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            is_h2c_preface(&stream).await.unwrap()
        });

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .await
            .unwrap();
        client.flush().await.unwrap();

        assert!(!server.await.unwrap());
    }

    // ==================== Absolute-form HTTP request building tests ====================

    fn make_request_parts(method: Method, uri: &str) -> http::request::Parts {
        let req = http::Request::builder()
            .method(method)
            .uri(uri)
            .body(())
            .unwrap();
        req.into_parts().0
    }

    #[test]
    fn build_absolute_http_request_creates_correct_format() {
        let parts = make_request_parts(Method::GET, "/api/users?page=1");

        let destination = EndpointAuthority {
            endpoint_id: iroh::EndpointId::from_bytes(&[0u8; 32]).unwrap(),
            authority: Authority {
                host: "localhost".to_string(),
                port: 5173,
            },
        };

        let headers = HeaderMap::new();
        let body = b"";

        let request = build_absolute_http_request(&parts, &destination, body, &headers);
        let request_str = String::from_utf8(request).unwrap();

        // Check the request line uses absolute-form URL
        assert!(
            request_str.starts_with("GET http://localhost:5173/api/users?page=1 HTTP/1.1\r\n"),
            "Request should use absolute-form URL, got: {}",
            request_str.lines().next().unwrap()
        );

        // Check Host header is present
        assert!(
            request_str.contains("Host: localhost:5173\r\n"),
            "Request should have Host header"
        );

        // Check Content-Length header
        assert!(
            request_str.contains("Content-Length: 0\r\n"),
            "Request should have Content-Length header"
        );

        // Check ends with double CRLF
        assert!(
            request_str.ends_with("\r\n\r\n"),
            "Request should end with blank line"
        );
    }

    #[test]
    fn build_absolute_http_request_with_body() {
        let parts = make_request_parts(Method::POST, "/api/data");

        let destination = EndpointAuthority {
            endpoint_id: iroh::EndpointId::from_bytes(&[0u8; 32]).unwrap(),
            authority: Authority {
                host: "127.0.0.1".to_string(),
                port: 8080,
            },
        };

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let body = b"{\"key\": \"value\"}";

        let request = build_absolute_http_request(&parts, &destination, body, &headers);
        let request_str = String::from_utf8(request).unwrap();

        // Check method and absolute URL
        assert!(request_str.starts_with("POST http://127.0.0.1:8080/api/data HTTP/1.1\r\n"));

        // Check content-type is preserved
        assert!(request_str.contains("content-type: application/json\r\n"));

        // Check Content-Length matches body length
        assert!(request_str.contains("Content-Length: 16\r\n"));
    }

    #[test]
    fn build_absolute_http_request_filters_hop_by_hop_headers() {
        let parts = make_request_parts(Method::GET, "/");

        let destination = EndpointAuthority {
            endpoint_id: iroh::EndpointId::from_bytes(&[0u8; 32]).unwrap(),
            authority: Authority {
                host: "localhost".to_string(),
                port: 80,
            },
        };

        let mut headers = HeaderMap::new();
        // These should be filtered out
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("x-iroh-endpoint-id", HeaderValue::from_static("abc123"));
        // This should be preserved
        headers.insert("x-custom-header", HeaderValue::from_static("preserved"));

        let request = build_absolute_http_request(&parts, &destination, b"", &headers);
        let request_str = String::from_utf8(request).unwrap();

        // Hop-by-hop headers should be removed
        assert!(!request_str.contains("connection:"));
        assert!(!request_str.contains("transfer-encoding:"));
        assert!(!request_str.contains("x-iroh-endpoint-id:"));

        // Custom header should be preserved
        assert!(request_str.contains("x-custom-header: preserved\r\n"));
    }

    // ==================== HTTP response parsing tests ====================

    #[test]
    fn find_header_end_finds_crlf_crlf() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(find_header_end(data), Some(38));
    }

    #[test]
    fn find_header_end_returns_none_when_incomplete() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n";
        assert_eq!(find_header_end(data), None);
    }

    #[test]
    fn content_length_parses_header() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", HeaderValue::from_static("1234"));
        assert_eq!(content_length(&headers), Some(1234));
    }

    #[test]
    fn content_length_returns_none_when_missing() {
        let headers = HeaderMap::new();
        assert_eq!(content_length(&headers), None);
    }

    #[test]
    fn is_chunked_detects_chunked_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        assert!(is_chunked(&headers));
    }

    #[test]
    fn is_chunked_handles_mixed_case() {
        let mut headers = HeaderMap::new();
        headers.insert("transfer-encoding", HeaderValue::from_static("Chunked"));
        assert!(is_chunked(&headers));
    }

    #[test]
    fn is_chunked_returns_false_for_other_encodings() {
        let mut headers = HeaderMap::new();
        headers.insert("transfer-encoding", HeaderValue::from_static("gzip"));
        assert!(!is_chunked(&headers));
    }
}
