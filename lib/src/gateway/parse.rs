use axum::http;
use n0_error::{StackResultExt, StdResultExt};
use tokio::io::{self, AsyncRead, AsyncReadExt};
use unicase::UniCase;

#[derive(derive_more::Debug)]
pub(super) struct PartialHttpRequest {
    #[allow(unused)]
    pub(super) method: http::Method,
    #[allow(unused)]
    pub(super) path: Option<String>,
    pub(super) host: String,
    pub(super) headers: http::HeaderMap<String>,
    #[debug(skip)]
    pub(super) initial_data: Vec<u8>,
}

impl PartialHttpRequest {
    pub(super) async fn read(
        reader: &mut (impl AsyncRead + Unpin),
        header_names: impl IntoIterator<Item = &str>,
    ) -> n0_error::Result<Self> {
        fn header<'a>(req: &'a httparse::Request, name: &str) -> Option<&'a str> {
            req.headers
                .iter()
                .find(|h| UniCase::new(h.name) == UniCase::new(name))
                .and_then(|h| std::str::from_utf8(h.value).ok())
        }

        let initial_data = read_headers(reader, 8192).await?;
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);

        match req
            .parse(&initial_data)
            .std_context("Failed to parse HTTP request")?
        {
            httparse::Status::Complete(_bytes_parsed) => {
                let method = req
                    .method
                    .context("Invalid HTTP request: Missing HTTP method")?;
                let method = method
                    .parse()
                    .std_context("Invalid HTTP request: Invalid method")?;
                let host = header(&req, http::header::HOST.as_str())
                    .context("Invalid HTTP request: Missing host header")?;
                let headers =
                    http::HeaderMap::from_iter(header_names.into_iter().flat_map(|name| {
                        let value = header(&req, name)?;
                        let key = http::HeaderName::from_bytes(name.as_bytes()).ok()?;
                        Some((key, value.to_string()))
                    }));
                Ok(Self {
                    path: req.path.map(ToOwned::to_owned),
                    method,
                    host: host.to_string(),
                    headers,
                    initial_data,
                })
            }
            httparse::Status::Partial => Err(n0_error::AnyError::from_string(
                "Incomplete HTTP request".to_string(),
            )),
        }
    }
}

async fn read_headers(
    reader: &mut (impl AsyncRead + Unpin),
    max_len: usize,
) -> io::Result<Vec<u8>> {
    const SEPARATOR: &[u8; 4] = b"\r\n\r\n";
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];

    while buf.len() < max_len {
        let n = reader.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
        }
        buf.extend_from_slice(&tmp[..n.min(max_len - buf.len())]);

        if let Some(_i) = buf.windows(SEPARATOR.len()).position(|w| w == SEPARATOR) {
            // buf.truncate(i + SEPARATOR.len());
            return Ok(buf);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid HTTP request: missing empty line after headers",
    ))
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
