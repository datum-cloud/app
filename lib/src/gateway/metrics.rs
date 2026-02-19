use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{Router, extract::State, routing::get};
use hyper::http::header;
use iroh::Endpoint;
use iroh_metrics::Registry;
use n0_error::Result;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Default)]
pub(super) struct GatewayMetrics {
    requests_tunnel_total: AtomicU64,
    requests_origin_total: AtomicU64,
    denied_missing_header_total: AtomicU64,
    denied_invalid_endpoint_total: AtomicU64,
    denied_invalid_target_port_total: AtomicU64,
    responses_4xx_total: AtomicU64,
    responses_5xx_total: AtomicU64,
}

impl GatewayMetrics {
    pub(super) fn inc_tunnel_requests(&self) {
        self.requests_tunnel_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_origin_requests(&self) {
        self.requests_origin_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_denied_missing_header(&self) {
        self.denied_missing_header_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_denied_invalid_endpoint(&self) {
        self.denied_invalid_endpoint_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_denied_invalid_target_port(&self) {
        self.denied_invalid_target_port_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_status_code(&self, status: hyper::StatusCode) {
        if status.is_client_error() {
            self.responses_4xx_total.fetch_add(1, Ordering::Relaxed);
        } else if status.is_server_error() {
            self.responses_5xx_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn render(&self, endpoint: &Endpoint) -> String {
        let endpoint_metrics = endpoint.metrics();
        let direct_added = endpoint_metrics.magicsock.num_direct_conns_added.get();
        let direct_removed = endpoint_metrics.magicsock.num_direct_conns_removed.get();
        let relay_added = endpoint_metrics.magicsock.num_relay_conns_added.get();
        let relay_removed = endpoint_metrics.magicsock.num_relay_conns_removed.get();
        let direct_current = direct_added.saturating_sub(direct_removed);
        let relay_current = relay_added.saturating_sub(relay_removed);
        let recv_total = endpoint_metrics.magicsock.recv_data_ipv4.get()
            + endpoint_metrics.magicsock.recv_data_ipv6.get()
            + endpoint_metrics.magicsock.recv_data_relay.get();
        let send_total = endpoint_metrics.magicsock.send_data.get();
        let mut endpoint_openmetrics = String::new();
        let mut registry = Registry::default();
        registry
            .sub_registry_with_prefix("iroh_gateway_endpoint")
            .register_all(endpoint.metrics());
        let _ = registry.encode_openmetrics_to_writer(&mut endpoint_openmetrics);

        format!(
            concat!(
                "# HELP iroh_gateway_requests_total Gateway request count by proxy request kind.\n",
                "# TYPE iroh_gateway_requests_total counter\n",
                "iroh_gateway_requests_total{{kind=\"tunnel\"}} {}\n",
                "iroh_gateway_requests_total{{kind=\"origin\"}} {}\n",
                "# HELP iroh_gateway_denied_requests_total Gateway denied request count by reason.\n",
                "# TYPE iroh_gateway_denied_requests_total counter\n",
                "iroh_gateway_denied_requests_total{{reason=\"missing_header\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"invalid_endpoint_id\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"invalid_target_port\"}} {}\n",
                "# HELP iroh_gateway_error_responses_total Gateway error response count grouped by status class.\n",
                "# TYPE iroh_gateway_error_responses_total counter\n",
                "iroh_gateway_error_responses_total{{class=\"4xx\"}} {}\n",
                "iroh_gateway_error_responses_total{{class=\"5xx\"}} {}\n",
                "# HELP iroh_gateway_iroh_recv_bytes_total Total iroh magicsock bytes received.\n",
                "# TYPE iroh_gateway_iroh_recv_bytes_total counter\n",
                "iroh_gateway_iroh_recv_bytes_total {}\n",
                "# HELP iroh_gateway_iroh_send_bytes_total Total iroh magicsock bytes sent.\n",
                "# TYPE iroh_gateway_iroh_send_bytes_total counter\n",
                "iroh_gateway_iroh_send_bytes_total {}\n\n",
                "# HELP iroh_gateway_quic_connections_opened_total QUIC peer connections opened by transport path.\n",
                "# TYPE iroh_gateway_quic_connections_opened_total counter\n",
                "iroh_gateway_quic_connections_opened_total{{path=\"direct\"}} {}\n",
                "iroh_gateway_quic_connections_opened_total{{path=\"relay\"}} {}\n",
                "# HELP iroh_gateway_quic_connections_closed_total QUIC peer connections closed by transport path.\n",
                "# TYPE iroh_gateway_quic_connections_closed_total counter\n",
                "iroh_gateway_quic_connections_closed_total{{path=\"direct\"}} {}\n",
                "iroh_gateway_quic_connections_closed_total{{path=\"relay\"}} {}\n",
                "# HELP iroh_gateway_quic_connections_current Current QUIC peer connections by transport path.\n",
                "# TYPE iroh_gateway_quic_connections_current gauge\n",
                "iroh_gateway_quic_connections_current{{path=\"direct\"}} {}\n",
                "iroh_gateway_quic_connections_current{{path=\"relay\"}} {}\n\n",
            ),
            self.requests_tunnel_total.load(Ordering::Relaxed),
            self.requests_origin_total.load(Ordering::Relaxed),
            self.denied_missing_header_total.load(Ordering::Relaxed),
            self.denied_invalid_endpoint_total.load(Ordering::Relaxed),
            self.denied_invalid_target_port_total
                .load(Ordering::Relaxed),
            self.responses_4xx_total.load(Ordering::Relaxed),
            self.responses_5xx_total.load(Ordering::Relaxed),
            recv_total,
            send_total,
            direct_added,
            relay_added,
            direct_removed,
            relay_removed,
            direct_current,
            relay_current,
        ) + &endpoint_openmetrics
    }
}

#[derive(Clone)]
pub(super) struct MetricsHttpState {
    endpoint: Endpoint,
    metrics: Arc<GatewayMetrics>,
}

impl MetricsHttpState {
    pub(super) fn new(endpoint: Endpoint, metrics: Arc<GatewayMetrics>) -> Self {
        Self { endpoint, metrics }
    }
}

pub(super) async fn serve_metrics_http(addr: SocketAddr, state: MetricsHttpState) -> Result<()> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state);
    let listener = TcpListener::bind(addr).await?;
    info!(metrics_bind_addr = %addr, "gateway metrics server started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler(
    State(state): State<MetricsHttpState>,
) -> ([(header::HeaderName, &'static str); 1], String) {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(&state.endpoint),
    )
}
