//! Command line arguments.
use clap::{Parser, Subcommand, ValueEnum};
mod dns_dev;
mod tunnel_dev;

use lib::{
    Advertisment, AdvertismentTicket, ConnectNode, DiscoveryMode, ListenNode, ProxyState, Repo,
    TcpProxyData,
    datum_cloud::{ApiEnv, DatumCloudClient},
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};
use tracing::info;

/// Datum Connect Agent
#[derive(Parser, Debug)]
struct Args {
    #[clap(short, long, env = "DATUM_CONNECT_REPO")]
    repo: Option<PathBuf>,
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a tunnel server that exposes configured local services through the Datum gateway.
    Serve,

    /// Join a proxy, i.e. connect to the proxy and expose the service locally.
    Connect(ConnectArgs),

    /// Start a gateway server that forwards HTTP requests through a Datum Connect tunnel.
    Gateway(ServeArgs),

    /// Run a local DNS server for development TXT records.
    #[clap(subcommand)]
    DnsDev(DnsDevArgs),

    /// Local entrypoint that tunnels traffic through the gateway using CONNECT.
    TunnelDev(TunnelDevArgs),

    /// List configured proxies.
    List,

    /// Add proxies.
    #[clap(subcommand, alias = "ls")]
    Add(AddCommands),
}

#[derive(Debug, clap::Parser)]
enum AddCommands {
    TcpProxy {
        host: String,
        #[clap(long)]
        label: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum DnsDevArgs {
    /// Serve a local DNS responder for _iroh TXT records.
    Serve(DnsDevServeArgs),
    /// Upsert a TXT record into the dev config file.
    Upsert(DnsDevUpsertArgs),
}

#[derive(Parser, Debug)]
pub struct DnsDevServeArgs {
    /// UDP bind address for the DNS server.
    #[clap(long, default_value = "127.0.0.1:53535")]
    pub bind: SocketAddr,
    /// Origin domain for _iroh.<z32>.<origin>.
    #[clap(long)]
    pub origin: String,
    /// Path to the YAML config file containing records.
    #[clap(long, default_value = "dns-dev.yml")]
    pub data: PathBuf,
    /// Reload interval for reading updated config file.
    #[clap(long, default_value = "1s")]
    pub reload_interval: humantime::Duration,
}

#[derive(Parser, Debug)]
pub struct DnsDevUpsertArgs {
    /// Origin domain for _iroh.<z32>.<origin>.
    #[clap(long)]
    pub origin: String,
    /// Path to the YAML config file containing records.
    #[clap(long, default_value = "dns-dev.yml")]
    pub data: PathBuf,
    /// EndpointId for the TXT record (iroh public key).
    #[clap(long)]
    pub endpoint_id: String,
    /// Optional relay URL.
    #[clap(long)]
    pub relay: Option<String>,
    /// Direct socket addresses for the endpoint (repeatable).
    #[clap(long)]
    pub addr: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct TunnelDevArgs {
    /// TCP bind address for local browser traffic.
    #[clap(long, default_value = "127.0.0.1:8888")]
    pub listen: SocketAddr,
    /// Gateway address that accepts CONNECT requests.
    #[clap(long, default_value = "127.0.0.1:8080")]
    pub gateway: SocketAddr,
    /// iroh endpoint id for the connector.
    #[clap(long)]
    pub node_id: String,
    /// Target host to dial through the tunnel.
    #[clap(long, default_value = "127.0.0.1")]
    pub target_host: String,
    /// Target port to dial through the tunnel.
    #[clap(long)]
    pub target_port: u16,
    /// Target protocol (must be tcp for now).
    #[clap(long, default_value = "tcp")]
    pub target_protocol: String,
}

#[derive(Parser, Debug)]
pub struct ConnectArgs {
    /// The addresses to listen on for incoming tcp connections.
    ///
    /// If unset uses the addr provided in the advertisment.
    ///
    /// To listen on all network interfaces, use 0.0.0.0:12345
    #[clap(long)]
    pub bind: SocketAddr,

    /// provide a ticket to drive connection directly.
    #[clap(long, conflicts_with = "codename")]
    pub ticket: AdvertismentTicket,
}

#[derive(Parser, Debug)]
pub struct ServeArgs {
    #[clap(long, default_value = "0.0.0.0")]
    pub bind_addr: IpAddr,
    #[clap(long, default_value = "8080")]
    pub port: u16,
    /// Optional bind address for Prometheus metrics server.
    #[clap(long)]
    pub metrics_addr: Option<IpAddr>,
    /// Optional port for Prometheus metrics server.
    #[clap(long)]
    pub metrics_port: Option<u16>,
    /// Also listen on a Unix domain socket at this path (e.g. for Envoy to forward via UDS).
    #[cfg(unix)]
    #[clap(long)]
    pub uds: Option<PathBuf>,
    /// Discovery mode for connection details.
    #[clap(long, value_enum)]
    pub discovery: Option<DiscoveryModeArg>,
    /// DNS origin for _iroh.<endpoint-id>.<origin> lookups.
    #[clap(long)]
    pub dns_origin: Option<String>,
    /// DNS resolver address for discovery (e.g. 127.0.0.1:53535).
    #[clap(long)]
    pub dns_resolver: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum GatewayModeArg {
    Reverse,
    Forward,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DiscoveryModeArg {
    Default,
    Dns,
    Hybrid,
}

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    tracing_subscriber::fmt::init();
    if let Ok(path) = dotenv::dotenv() {
        info!("Loaded environment variables from {}", path.display());
    }

    let args = Args::parse();

    let path = args.repo.unwrap_or_else(Repo::default_location);
    let repo = Repo::open_or_create(path).await?;

    match args.command {
        Commands::List => {
            let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;
            let orgs = datum.orgs_and_projects().await?;
            for org in orgs {
                println!("org: {} {}", org.org.resource_id, org.org.display_name);
                for project in org.projects {
                    println!(
                        "  project: {} {}",
                        project.resource_id, project.display_name
                    );
                }
            }

            println!();
            let state = repo.load_state().await?;
            for p in state.get().proxies.iter() {
                println!(
                    "{} -> {}:{} (enabled: {})",
                    p.info.resource_id, p.info.data.host, p.info.data.port, p.enabled
                )
            }
        }
        Commands::Add(AddCommands::TcpProxy { host, label }) => {
            let service = TcpProxyData::from_host_port_str(&host)?;
            let advertisment = Advertisment::new(service, label);
            let proxy = ProxyState {
                enabled: true,
                info: advertisment,
            };

            println!("Adding {proxy:?})");
            let state = repo.load_state().await?;
            state
                .update(&repo, |state| {
                    state.set_proxy(proxy);
                })
                .await?;
            println!("OK.");
        }
        Commands::Serve => {
            let node = ListenNode::new(repo).await?;
            let endpoint_id = node.endpoint_id();
            println!("listening as {}", endpoint_id);
            let bound_addrs = node.endpoint().bound_sockets();
            if !bound_addrs.is_empty() {
                println!("iroh bound sockets:");
                for addr in &bound_addrs {
                    println!("  {addr}");
                }
                let z32_id = z32::encode(endpoint_id.as_bytes());
                println!();
                println!("dns-dev lookup:");
                println!("  _iroh.{z32_id}.datumconnect.test");
                println!();
                println!("dns-dev example:");
                println!(
                    "  datum-connect dns-dev upsert --origin datumconnect.test --data ./dns-dev.yml --endpoint-id {} --addr {}",
                    endpoint_id,
                    bound_addrs
                        .iter()
                        .map(|addr| addr.to_string())
                        .collect::<Vec<_>>()
                        .join(" --addr ")
                );
            }
            for p in node.proxies() {
                if !p.enabled {
                    continue;
                };
                println!(
                    "{} -> {}:{}",
                    p.info.resource_id, p.info.data.host, p.info.data.port
                )
            }
            tokio::signal::ctrl_c().await?;
            println!()
        }
        Commands::Connect(args) => {
            let ConnectArgs { bind, ticket } = args;
            let node = ConnectNode::new(repo).await?;

            let handle = node
                .connect_and_bind_local(ticket.endpoint, &ticket.data.data, bind)
                .await?;
            println!(
                "server listening on {}, forwarding connections to {} -> {}:{}",
                handle.bound_addr(),
                handle.remote_id().fmt_short(),
                handle.advertisment().host,
                handle.advertisment().port,
            );
            tokio::signal::ctrl_c().await?;
            handle.abort();
        }
        Commands::Gateway(args) => {
            let bind_addr: SocketAddr = (args.bind_addr, args.port).into();
            let metrics_bind_addr = match (args.metrics_addr, args.metrics_port) {
                (None, None) => None,
                (Some(addr), Some(port)) => Some((addr, port).into()),
                (Some(addr), None) => Some((addr, 9090).into()),
                (None, Some(port)) => Some((IpAddr::from([127, 0, 0, 1]), port).into()),
            };
            let secret_key = repo.gateway_key().await?;
            let mut config = repo.gateway_config().await?;
            if let Some(discovery) = args.discovery {
                config.common.discovery_mode = match discovery {
                    DiscoveryModeArg::Default => DiscoveryMode::Default,
                    DiscoveryModeArg::Dns => DiscoveryMode::Dns,
                    DiscoveryModeArg::Hybrid => DiscoveryMode::Hybrid,
                };
            }
            if let Some(origin) = args.dns_origin {
                config.common.dns_origin = Some(origin);
            }
            if let Some(resolver) = args.dns_resolver {
                config.common.dns_resolver = Some(resolver);
            }
            #[cfg(unix)]
            if let Some(uds_path) = &args.uds {
                let sk = secret_key.clone();
                let cfg = config.clone();
                let path = uds_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = lib::gateway::bind_and_serve_uds(sk, cfg, path).await {
                        tracing::warn!(%e, "UDS gateway task failed");
                    }
                });
                println!("UDS gateway at {}", uds_path.display());
            }
            println!("serving on port {bind_addr}");
            tokio::select! {
                res = lib::gateway::bind_and_serve(secret_key, config, bind_addr, metrics_bind_addr) => res?,
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Commands::DnsDev(args) => match args {
            DnsDevArgs::Serve(args) => {
                dns_dev::serve(
                    args.bind,
                    args.data,
                    args.origin,
                    args.reload_interval.into(),
                )
                .await?;
            }
            DnsDevArgs::Upsert(args) => {
                dns_dev::upsert(
                    args.data,
                    args.origin,
                    args.endpoint_id,
                    args.relay,
                    args.addr,
                )?;
            }
        },
        Commands::TunnelDev(args) => {
            tunnel_dev::serve(args).await?;
        }
    }
    Ok(())
}
