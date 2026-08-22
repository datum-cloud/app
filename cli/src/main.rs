//! Command line arguments.
use clap::{Parser, Subcommand};
mod cloud;
mod dns_dev;
mod tunnel_dev;

use lib::{
    Advertisment, AdvertismentTicket, ConnectNode, ListenNode, ProxyState, Repo, TcpProxyData,
    datum_cloud::{ApiEnv, DatumCloudClient},
};
use std::{net::SocketAddr, path::PathBuf};
use tracing::info;
use tracing_subscriber::{EnvFilter, prelude::*};

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
    /// Log in to Datum Cloud via the browser.
    Login {
        /// Re-run the browser login even if a valid session exists.
        #[clap(long)]
        force: bool,
        #[clap(long)]
        json: bool,
    },
    /// Clear the stored Datum Cloud session.
    Logout {
        #[clap(long)]
        json: bool,
    },
    /// Show login, selected project, and agent status.
    Status {
        #[clap(long)]
        json: bool,
    },
    /// Select the Datum Cloud organization and project used for tunnels.
    #[clap(subcommand)]
    Context(ContextCommands),
    /// Run or inspect the headless listener that serves tunnels.
    #[clap(subcommand)]
    Agent(AgentCommands),
    /// Create and manage public tunnels through the running agent.
    #[clap(subcommand)]
    Tunnel(TunnelCommands),

    /// Start a tunnel server that exposes configured local services through the Datum gateway.
    Serve,

    /// Join a proxy, i.e. connect to the proxy and expose the service locally.
    Connect(ConnectArgs),

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

#[derive(Subcommand, Debug)]
pub(crate) enum ContextCommands {
    /// Show the currently selected organization and project.
    Show {
        #[clap(long)]
        json: bool,
    },
    /// List organizations and projects.
    List {
        #[clap(long)]
        json: bool,
    },
    /// Select a project by id or name.
    Set {
        /// Project resource id or display name.
        #[clap(long)]
        project: String,
        /// Organization resource id or display name (required when the project name is ambiguous).
        #[clap(long)]
        org: Option<String>,
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AgentCommands {
    /// Start the agent in the foreground.
    Start,
    /// Stop a running agent.
    Stop,
    /// Show whether the agent is running.
    Status {
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum TunnelCommands {
    /// Create a tunnel to a local HTTP endpoint.
    Create {
        /// Display name for the tunnel.
        #[clap(long)]
        label: String,
        /// Local host:port or URL, e.g. 127.0.0.1:4123.
        #[clap(long)]
        endpoint: String,
        /// Block until Datum assigns a public hostname.
        #[clap(long)]
        wait_hostname: bool,
        /// How long to wait for a hostname when --wait-hostname is set.
        #[clap(long, default_value = "30s")]
        timeout: humantime::Duration,
        #[clap(long)]
        json: bool,
    },
    /// List tunnels in the selected project.
    List {
        #[clap(long)]
        json: bool,
    },
    /// Show one tunnel.
    Get {
        id: String,
        #[clap(long)]
        json: bool,
    },
    /// Delete a tunnel.
    Delete {
        id: String,
        #[clap(long)]
        json: bool,
    },
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

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    // Required before any TLS use (kube, reqwest, iroh). The GUI does the same
    // in ui/src/main.rs; without it the agent panics in rustls when creating a tunnel.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("rustls default crypto provider");

    if lib::agent::wants_headless_agent() {
        return lib::agent::run_headless_agent_from_args().await;
    }

    // Load .env first so any process-env-driven config is visible to the rest
    // of init. We keep the load result so we can log it *after* tracing is up.
    let dotenv_path = dotenv::dotenv().ok();

    // Initialize Sentry before tracing so the tracing layer registered below
    // dispatches to a real Hub from the first event onwards. SENTRY_DSN is
    // baked at compile time via `option_env!`; dev builds typically have no
    // DSN, so Sentry naturally runs as a no-op outside release builds.
    let _sentry_guard = sentry::init(sentry::ClientOptions {
        dsn: option_env!("SENTRY_DSN")
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        release: sentry::release_name!(),
        send_default_pii: true,
        traces_sample_rate: 0.1,
        ..Default::default()
    });

    // Promote WARN to a Sentry event in addition to the default ERROR -> Event
    // and INFO -> Breadcrumb. See ui/src/main.rs for rationale.
    let sentry_layer = sentry::integrations::tracing::layer().event_filter(|md| {
        use sentry::integrations::tracing::EventFilter;
        match *md.level() {
            tracing::Level::ERROR => EventFilter::Event | EventFilter::Breadcrumb,
            tracing::Level::WARN => EventFilter::Event | EventFilter::Breadcrumb,
            tracing::Level::INFO => EventFilter::Breadcrumb,
            _ => EventFilter::Ignore,
        }
    });

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(sentry_layer)
        .init();

    if let Some(path) = dotenv_path {
        info!("Loaded environment variables from {}", path.display());
    }

    let args = Args::parse();

    let path = args.repo.unwrap_or_else(Repo::default_location);
    let repo = Repo::open_or_create(path).await?;

    match args.command {
        Commands::Login { force, json } => cloud::login(repo, force, json).await?,
        Commands::Logout { json } => cloud::logout(repo, json).await?,
        Commands::Status { json } => cloud::status(repo, json).await?,
        Commands::Context(command) => cloud::context(repo, command).await?,
        Commands::Agent(command) => cloud::agent(repo, command).await?,
        Commands::Tunnel(command) => cloud::tunnel(repo, command).await?,
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
