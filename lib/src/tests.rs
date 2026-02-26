use std::net::Ipv4Addr;

use http_body_util::BodyExt;
use hyper::{Request, StatusCode, client::conn::http2};
use hyper_util::rt::{TokioExecutor, TokioIo};
use iroh::{Endpoint, discovery::static_provider::StaticProvider};
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use n0_tracing_test::traced_test;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::{Advertisment, ListenNode, ProxyState, Repo, TcpProxyData, gateway};

#[derive(Default)]
struct TestDiscovery(StaticProvider);

impl TestDiscovery {
    fn add(&self, endpoint: &Endpoint) {
        endpoint.discovery().add(self.0.clone());
        self.0.add_endpoint_info(endpoint.addr());
    }
}

#[tokio::test]
#[traced_test]
async fn gateway_end_to_end_to_upstream_http() -> Result<()> {
    let discovery = TestDiscovery::default();

    let temp_dir = tempfile::tempdir()?;
    let repo = Repo::open_or_create(temp_dir.path()).await?;

    let (origin_addr, _origin_task) = origin_server::spawn("origin").await?;

    let proxy_state = {
        let data = TcpProxyData::from_host_port_str(&origin_addr.to_string())?;
        let advertisment = Advertisment::new(data, None);
        ProxyState::new(advertisment)
    };

    let codename = proxy_state.info.codename();

    let upstream = ListenNode::new(repo).await?;
    discovery.add(upstream.endpoint());
    upstream.set_proxy(proxy_state).await?;

    let (gateway_addr, _gateway_task) = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let endpoint = Endpoint::bind().await?;
        discovery.add(&endpoint);
        let task = tokio::task::spawn(gateway::serve(endpoint, listener));
        (addr, AbortOnDropHandle::new(task))
    };

    let domain = format!("{codename}.localhost");
    let client = reqwest::Client::builder()
        .resolve_to_addrs(&domain, &[(Ipv4Addr::LOCALHOST, 0).into()])
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let res = client
        .get(format!(
            "http://{codename}.localhost:{}/hello",
            gateway_addr.port()
        ))
        .header("x-datum-target-host", origin_addr.ip().to_string())
        .header("x-datum-target-port", origin_addr.port().to_string())
        .header("x-iroh-endpoint-id", upstream.endpoint_id().to_string())
        .send()
        .await
        .anyerr()?;
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.text().await.anyerr()?;
    assert_eq!(body, "origin GET /hello");

    Ok(())
}

#[tokio::test]
#[traced_test]
async fn gateway_forward_connect_tunnel() -> Result<()> {
    let discovery = TestDiscovery::default();

    let temp_dir = tempfile::tempdir()?;
    let repo = Repo::open_or_create(temp_dir.path()).await?;

    let (origin_addr, _origin_task) = origin_server::spawn("origin").await?;

    let proxy_state = {
        let data = TcpProxyData::from_host_port_str(&origin_addr.to_string())?;
        let advertisment = Advertisment::new(data, None);
        ProxyState::new(advertisment)
    };

    let upstream = ListenNode::new(repo).await?;
    discovery.add(upstream.endpoint());
    upstream.set_proxy(proxy_state).await?;

    let (gateway_addr, _gateway_task) = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let endpoint = Endpoint::bind().await?;
        discovery.add(&endpoint);
        let task = tokio::task::spawn(gateway::serve(endpoint, listener));
        (addr, AbortOnDropHandle::new(task))
    };

    let mut stream = tokio::net::TcpStream::connect(gateway_addr).await?;
    let connect_request = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nx-iroh-endpoint-id: {node_id}\r\n\r\n",
        target = origin_addr,
        node_id = upstream.endpoint_id(),
    );
    stream.write_all(connect_request.as_bytes()).await?;

    let mut response = String::new();
    let mut buffer = [0u8; 512];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        response.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if response.contains("\r\n\r\n") {
            break;
        }
    }
    assert!(
        response.contains("200"),
        "unexpected CONNECT response: {response}"
    );

    stream
        .write_all(b"GET /hello HTTP/1.1\r\nHost: origin\r\n\r\n")
        .await?;
    let mut body = vec![0u8; 1024];
    let read = stream.read(&mut body).await?;
    let body = String::from_utf8_lossy(&body[..read]);
    assert!(
        body.contains("origin GET /hello"),
        "unexpected tunneled response: {body}"
    );

    Ok(())
}

#[tokio::test]
#[traced_test]
async fn gateway_forward_h2c_requests_are_stable() -> Result<()> {
    let discovery = TestDiscovery::default();

    let temp_dir = tempfile::tempdir()?;
    let repo = Repo::open_or_create(temp_dir.path()).await?;

    let (origin_addr, _origin_task) = origin_server::spawn("origin").await?;

    let proxy_state = {
        let data = TcpProxyData::from_host_port_str(&origin_addr.to_string())?;
        let advertisment = Advertisment::new(data, None);
        ProxyState::new(advertisment)
    };

    let upstream = ListenNode::new(repo).await?;
    discovery.add(upstream.endpoint());
    upstream.set_proxy(proxy_state).await?;

    let (gateway_addr, _gateway_task) = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let endpoint = Endpoint::bind().await?;
        discovery.add(&endpoint);
        let task = tokio::task::spawn(gateway::serve(endpoint, listener));
        (addr, AbortOnDropHandle::new(task))
    };

    let stream = tokio::net::TcpStream::connect(gateway_addr).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = http2::Builder::new(TokioExecutor::new())
        .handshake(io)
        .await
        .map_err(|err| n0_error::anyerr!(err))?;
    tokio::spawn(async move {
        if let Err(err) = conn.await {
            tracing::warn!("h2c client connection error: {err:#}");
        }
    });

    for _ in 0..5 {
        let req: Request<http_body_util::Full<hyper::body::Bytes>> = Request::builder()
            .method("GET")
            .uri("/hello")
            .header("x-iroh-endpoint-id", upstream.endpoint_id().to_string())
            .header("x-datum-target-host", origin_addr.ip().to_string())
            .header("x-datum-target-port", origin_addr.port().to_string())
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .unwrap();

        let res = sender
            .send_request(req)
            .await
            .map_err(|err| n0_error::anyerr!(err))?;
        assert_eq!(res.status(), StatusCode::OK);
        let body = res
            .into_body()
            .collect()
            .await
            .map_err(|err| n0_error::anyerr!(err))?
            .to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("origin GET /hello"),
            "unexpected h2c response: {body}"
        );
    }

    Ok(())
}

#[tokio::test]
#[traced_test]
async fn gateway_forward_h2c_handles_closed_origin_connections() -> Result<()> {
    let discovery = TestDiscovery::default();

    let temp_dir = tempfile::tempdir()?;
    let repo = Repo::open_or_create(temp_dir.path()).await?;

    let (origin_addr, _origin_task) = origin_server::spawn_closing("origin").await?;

    let proxy_state = {
        let data = TcpProxyData::from_host_port_str(&origin_addr.to_string())?;
        let advertisment = Advertisment::new(data, None);
        ProxyState::new(advertisment)
    };

    let upstream = ListenNode::new(repo).await?;
    discovery.add(upstream.endpoint());
    upstream.set_proxy(proxy_state).await?;

    let (gateway_addr, _gateway_task) = {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let endpoint = Endpoint::bind().await?;
        discovery.add(&endpoint);
        let task = tokio::task::spawn(gateway::serve(endpoint, listener));
        (addr, AbortOnDropHandle::new(task))
    };

    let stream = tokio::net::TcpStream::connect(gateway_addr).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = http2::Builder::new(TokioExecutor::new())
        .handshake(io)
        .await
        .map_err(|err| n0_error::anyerr!(err))?;
    tokio::spawn(async move {
        if let Err(err) = conn.await {
            tracing::warn!("h2c client connection error: {err:#}");
        }
    });

    for _ in 0..3 {
        let req: Request<http_body_util::Full<hyper::body::Bytes>> = Request::builder()
            .method("GET")
            .uri("/hello")
            .header("x-iroh-endpoint-id", upstream.endpoint_id().to_string())
            .header("x-datum-target-host", origin_addr.ip().to_string())
            .header("x-datum-target-port", origin_addr.port().to_string())
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .unwrap();

        let res = sender
            .send_request(req)
            .await
            .map_err(|err| n0_error::anyerr!(err))?;
        assert_eq!(res.status(), StatusCode::OK);
        let body = res
            .into_body()
            .collect()
            .await
            .map_err(|err| n0_error::anyerr!(err))?
            .to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("origin GET /hello"),
            "unexpected h2c response: {body}"
        );
    }

    Ok(())
}

mod origin_server {
    use std::{convert::Infallible, net::SocketAddr, sync::Arc};

    use http_body_util::Full;
    use hyper::{Request, Response, body::Bytes, server::conn::http1, service::service_fn};
    use hyper_util::rt::TokioIo;
    use n0_future::task::AbortOnDropHandle;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use tracing::debug;

    /// Spawns a simple HTTP origin server that echoes back "{label} {method} {path}".
    pub async fn spawn(
        label: &'static str,
    ) -> n0_error::Result<(SocketAddr, AbortOnDropHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let tcp_addr = listener.local_addr()?;
        debug!(%label, %tcp_addr, "spawned origin server");
        let task = tokio::spawn(async move { run(listener, label).await });
        Ok((tcp_addr, AbortOnDropHandle::new(task)))
    }

    /// Spawns a raw HTTP/1.1 origin server that always closes after each response.
    pub async fn spawn_closing(
        label: &'static str,
    ) -> n0_error::Result<(SocketAddr, AbortOnDropHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let tcp_addr = listener.local_addr()?;
        debug!(%label, %tcp_addr, "spawned closing origin server");
        let task = tokio::spawn(async move { run_closing(listener, label).await });
        Ok((tcp_addr, AbortOnDropHandle::new(task)))
    }

    /// Returns "{label} {METHOD} {PATH}" as response body.
    pub(super) async fn run(listener: TcpListener, label: &'static str) {
        let label = Arc::new(label);
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let label = label.clone();
            tokio::task::spawn(async move {
                let handler = move |req: Request<hyper::body::Incoming>| {
                    let label = label.clone();
                    async move {
                        let body = format!("{} {} {}", *label, req.method(), req.uri().path());
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
                    }
                };
                let _ = http1::Builder::new()
                    .serve_connection(io, service_fn(handler))
                    .await;
            });
        }
    }

    async fn run_closing(listener: TcpListener, label: &'static str) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::task::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    let read = match stream.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = format!("{label} GET /hello");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    }
}
