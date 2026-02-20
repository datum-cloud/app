use std::{
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
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
    requests_tcp_total: AtomicU64,
    requests_uds_total: AtomicU64,
    requests_tunnel_tcp_total: AtomicU64,
    requests_tunnel_uds_total: AtomicU64,
    requests_origin_tcp_total: AtomicU64,
    requests_origin_uds_total: AtomicU64,
    tunnel_reuse_attempts_with_existing_peer_conn_total: AtomicU64,
    tunnel_reuse_attempts_without_existing_peer_conn_total: AtomicU64,
    origin_reuse_attempts_with_existing_peer_conn_total: AtomicU64,
    origin_reuse_attempts_without_existing_peer_conn_total: AtomicU64,
    denied_missing_header_total: AtomicU64,
    denied_missing_header_node_id_total: AtomicU64,
    denied_missing_header_target_host_total: AtomicU64,
    denied_missing_header_target_port_total: AtomicU64,
    denied_invalid_endpoint_total: AtomicU64,
    denied_invalid_target_port_total: AtomicU64,
    responses_4xx_total: AtomicU64,
    responses_5xx_total: AtomicU64,
    responses_500_total: AtomicU64,
    responses_502_total: AtomicU64,
    responses_503_total: AtomicU64,
    responses_504_total: AtomicU64,
    responses_other_5xx_total: AtomicU64,
    failures_5xx_with_existing_peer_conn_total: AtomicU64,
    failures_5xx_without_existing_peer_conn_total: AtomicU64,
}

static SHARED_METRICS: OnceLock<Arc<GatewayMetrics>> = OnceLock::new();

pub(super) fn shared_gateway_metrics() -> Arc<GatewayMetrics> {
    SHARED_METRICS
        .get_or_init(|| Arc::new(GatewayMetrics::default()))
        .clone()
}

impl GatewayMetrics {
    pub(super) fn inc_tunnel_requests(&self) {
        self.requests_tunnel_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_origin_requests(&self) {
        self.requests_origin_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_tunnel_reuse_attempt(&self, has_existing_peer_conn: bool) {
        if has_existing_peer_conn {
            self.tunnel_reuse_attempts_with_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.tunnel_reuse_attempts_without_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn inc_origin_reuse_attempt(&self, has_existing_peer_conn: bool) {
        if has_existing_peer_conn {
            self.origin_reuse_attempts_with_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.origin_reuse_attempts_without_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn inc_tunnel_tcp_requests(&self) {
        self.requests_tunnel_tcp_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(unix)]
    pub(super) fn inc_tunnel_uds_requests(&self) {
        self.requests_tunnel_uds_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_origin_tcp_requests(&self) {
        self.requests_origin_tcp_total
            .fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(unix)]
    pub(super) fn inc_origin_uds_requests(&self) {
        self.requests_origin_uds_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_tcp_requests(&self) {
        self.requests_tcp_total.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(unix)]
    pub(super) fn inc_uds_requests(&self) {
        self.requests_uds_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_denied_missing_header(&self) {
        self.denied_missing_header_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn inc_denied_missing_header_name(&self, name: &str) {
        self.inc_denied_missing_header();
        match name {
            "x-iroh-endpoint-id" => {
                self.denied_missing_header_node_id_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            "x-datum-target-host" => {
                self.denied_missing_header_target_host_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            "x-datum-target-port" => {
                self.denied_missing_header_target_port_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
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
            match status {
                hyper::StatusCode::INTERNAL_SERVER_ERROR => {
                    self.responses_500_total.fetch_add(1, Ordering::Relaxed);
                }
                hyper::StatusCode::BAD_GATEWAY => {
                    self.responses_502_total.fetch_add(1, Ordering::Relaxed);
                }
                hyper::StatusCode::SERVICE_UNAVAILABLE => {
                    self.responses_503_total.fetch_add(1, Ordering::Relaxed);
                }
                hyper::StatusCode::GATEWAY_TIMEOUT => {
                    self.responses_504_total.fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    self.responses_other_5xx_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pub(super) fn inc_5xx_failure_by_peer_conn_state(&self, has_existing_peer_conn: bool) {
        if has_existing_peer_conn {
            self.failures_5xx_with_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.failures_5xx_without_existing_peer_conn_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn render(&self, endpoint: &Endpoint) -> String {
        let endpoint_metrics = endpoint.metrics();
        let direct_added = endpoint_metrics.magicsock.num_direct_conns_added.get();
        let direct_removed = endpoint_metrics.magicsock.num_direct_conns_removed.get();
        let relay_added = endpoint_metrics.magicsock.num_relay_conns_added.get();
        let relay_removed = endpoint_metrics.magicsock.num_relay_conns_removed.get();
        let relay_send_errors = endpoint_metrics.magicsock.send_relay_error.get();
        let relay_home_changes = endpoint_metrics.magicsock.relay_home_change.get();
        let handshake_success = endpoint_metrics
            .magicsock
            .connection_handshake_success
            .get();
        let endpoints_contacted = endpoint_metrics.magicsock.endpoints_contacted.get();
        let endpoints_contacted_directly = endpoint_metrics
            .magicsock
            .endpoints_contacted_directly
            .get();
        let path_ping_failures = endpoint_metrics.magicsock.path_ping_failures.get();
        let path_marked_outdated = endpoint_metrics.magicsock.path_marked_outdated.get();
        let path_failure_resets = endpoint_metrics.magicsock.path_failure_resets.get();
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
                "# HELP iroh_gateway_requests_by_source_total Gateway request count by ingress source.\n",
                "# TYPE iroh_gateway_requests_by_source_total counter\n",
                "iroh_gateway_requests_by_source_total{{source=\"tcp\"}} {}\n",
                "iroh_gateway_requests_by_source_total{{source=\"uds\"}} {}\n",
                "# HELP iroh_gateway_requests_by_source_and_kind_total Gateway request count by ingress source and request kind.\n",
                "# TYPE iroh_gateway_requests_by_source_and_kind_total counter\n",
                "iroh_gateway_requests_by_source_and_kind_total{{source=\"tcp\",kind=\"tunnel\"}} {}\n",
                "iroh_gateway_requests_by_source_and_kind_total{{source=\"uds\",kind=\"tunnel\"}} {}\n",
                "iroh_gateway_requests_by_source_and_kind_total{{source=\"tcp\",kind=\"origin\"}} {}\n",
                "iroh_gateway_requests_by_source_and_kind_total{{source=\"uds\",kind=\"origin\"}} {}\n",
                "# HELP iroh_gateway_upstream_reuse_attempts_total Gateway upstream attempt count by request kind and whether a peer connection already existed.\n",
                "# TYPE iroh_gateway_upstream_reuse_attempts_total counter\n",
                "iroh_gateway_upstream_reuse_attempts_total{{kind=\"tunnel\",peer_conn_state=\"with_existing\"}} {}\n",
                "iroh_gateway_upstream_reuse_attempts_total{{kind=\"tunnel\",peer_conn_state=\"without_existing\"}} {}\n",
                "iroh_gateway_upstream_reuse_attempts_total{{kind=\"origin\",peer_conn_state=\"with_existing\"}} {}\n",
                "iroh_gateway_upstream_reuse_attempts_total{{kind=\"origin\",peer_conn_state=\"without_existing\"}} {}\n",
                "# HELP iroh_gateway_denied_requests_total Gateway denied request count by reason.\n",
                "# TYPE iroh_gateway_denied_requests_total counter\n",
                "iroh_gateway_denied_requests_total{{reason=\"missing_header\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"missing_header_node_id\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"missing_header_target_host\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"missing_header_target_port\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"invalid_endpoint_id\"}} {}\n",
                "iroh_gateway_denied_requests_total{{reason=\"invalid_target_port\"}} {}\n",
                "# HELP iroh_gateway_error_responses_total Gateway error response count grouped by status class.\n",
                "# TYPE iroh_gateway_error_responses_total counter\n",
                "iroh_gateway_error_responses_total{{class=\"4xx\"}} {}\n",
                "iroh_gateway_error_responses_total{{class=\"5xx\"}} {}\n",
                "# HELP iroh_gateway_error_responses_by_status_total Gateway 5xx response count grouped by exact status code.\n",
                "# TYPE iroh_gateway_error_responses_by_status_total counter\n",
                "iroh_gateway_error_responses_by_status_total{{status=\"500\"}} {}\n",
                "iroh_gateway_error_responses_by_status_total{{status=\"502\"}} {}\n",
                "iroh_gateway_error_responses_by_status_total{{status=\"503\"}} {}\n",
                "iroh_gateway_error_responses_by_status_total{{status=\"504\"}} {}\n",
                "iroh_gateway_error_responses_by_status_total{{status=\"other_5xx\"}} {}\n",
                "# HELP iroh_gateway_upstream_failures_total Gateway upstream 5xx failures grouped by whether a peer connection existed when the error was generated.\n",
                "# TYPE iroh_gateway_upstream_failures_total counter\n",
                "iroh_gateway_upstream_failures_total{{class=\"5xx\",peer_conn_state=\"with_existing\"}} {}\n",
                "iroh_gateway_upstream_failures_total{{class=\"5xx\",peer_conn_state=\"without_existing\"}} {}\n",
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
                "# HELP iroh_gateway_tunnel_connectivity_events_total Tunnel connectivity events from iroh magicsock state.\n",
                "# TYPE iroh_gateway_tunnel_connectivity_events_total counter\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"relay_send_error\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"relay_home_change\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"connection_handshake_success\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"endpoints_contacted\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"endpoints_contacted_directly\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"path_ping_failures\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"path_marked_outdated\"}} {}\n",
                "iroh_gateway_tunnel_connectivity_events_total{{event=\"path_failure_resets\"}} {}\n\n",
            ),
            self.requests_tunnel_total.load(Ordering::Relaxed),
            self.requests_origin_total.load(Ordering::Relaxed),
            self.requests_tcp_total.load(Ordering::Relaxed),
            self.requests_uds_total.load(Ordering::Relaxed),
            self.requests_tunnel_tcp_total.load(Ordering::Relaxed),
            self.requests_tunnel_uds_total.load(Ordering::Relaxed),
            self.requests_origin_tcp_total.load(Ordering::Relaxed),
            self.requests_origin_uds_total.load(Ordering::Relaxed),
            self.tunnel_reuse_attempts_with_existing_peer_conn_total
                .load(Ordering::Relaxed),
            self.tunnel_reuse_attempts_without_existing_peer_conn_total
                .load(Ordering::Relaxed),
            self.origin_reuse_attempts_with_existing_peer_conn_total
                .load(Ordering::Relaxed),
            self.origin_reuse_attempts_without_existing_peer_conn_total
                .load(Ordering::Relaxed),
            self.denied_missing_header_total.load(Ordering::Relaxed),
            self.denied_missing_header_node_id_total
                .load(Ordering::Relaxed),
            self.denied_missing_header_target_host_total
                .load(Ordering::Relaxed),
            self.denied_missing_header_target_port_total
                .load(Ordering::Relaxed),
            self.denied_invalid_endpoint_total.load(Ordering::Relaxed),
            self.denied_invalid_target_port_total
                .load(Ordering::Relaxed),
            self.responses_4xx_total.load(Ordering::Relaxed),
            self.responses_5xx_total.load(Ordering::Relaxed),
            self.responses_500_total.load(Ordering::Relaxed),
            self.responses_502_total.load(Ordering::Relaxed),
            self.responses_503_total.load(Ordering::Relaxed),
            self.responses_504_total.load(Ordering::Relaxed),
            self.responses_other_5xx_total.load(Ordering::Relaxed),
            self.failures_5xx_with_existing_peer_conn_total
                .load(Ordering::Relaxed),
            self.failures_5xx_without_existing_peer_conn_total
                .load(Ordering::Relaxed),
            recv_total,
            send_total,
            direct_added,
            relay_added,
            direct_removed,
            relay_removed,
            direct_current,
            relay_current,
            relay_send_errors,
            relay_home_changes,
            handshake_success,
            endpoints_contacted,
            endpoints_contacted_directly,
            path_ping_failures,
            path_marked_outdated,
            path_failure_resets,
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
