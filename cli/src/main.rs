//! Command line arguments.
use clap::{Parser, Subcommand, ValueEnum};
mod dns_dev;
mod tunnel_dev;

// Ensure rustls crypto provider is installed
use rustls::crypto::ring as rustls_ring;

use lib::{
    Advertisment, AdvertismentTicket, ConnectNode, DiscoveryMode, HeartbeatAgent, ListenNode,
    ProgressStepKind, ProxyState, Repo, SelectedContext, StepStatus, TcpProxyData, TunnelService,
    datum_cloud::{ApiEnv, DatumCloudClient, LoginState},
};
use n0_error::StdResultExt;
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use tracing::info;
use tracing_subscriber::prelude::*;

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

    /// Authenticate with Datum Cloud (login, logout, status).
    #[clap(subcommand)]
    Auth(AuthCommands),

    /// Manage tunnels (create, list, update, delete) that expose local services to public hostnames.
    Tunnel(TunnelArgs),

    /// Manage Datum Cloud projects.
    #[clap(subcommand)]
    Projects(ProjectsCommands),
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

#[derive(Subcommand, Debug)]
pub enum ProjectsCommands {
    /// List all available projects across your organizations.
    List,

    /// Switch the active project.
    Switch,
}

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Show current authentication status.
    Status,

    /// Log in to Datum Cloud (opens browser for OAuth).
    Login,

    /// Log out and clear stored credentials.
    Logout,

    /// List all locally authenticated users.
    List,

    /// Switch to a different authenticated user (clears current and prompts for new login).
    Switch,
}

#[derive(Parser, Debug)]
pub struct TunnelArgs {
    /// Project ID to use for this command (overrides the currently selected project).
    #[clap(long)]
    project: Option<String>,
    #[clap(subcommand)]
    command: TunnelCommands,
}

#[derive(Subcommand, Debug)]
pub enum TunnelCommands {
    /// List all tunnels in the current project.
    List,

    /// Start a tunnel that exposes a local service to a public hostname.
    Listen {
        /// Display name for the tunnel (auto-generated if not provided).
        #[clap(long)]
        label: Option<String>,
        /// Local address to expose (host:port, e.g. 127.0.0.1:8080).
        #[clap(long)]
        endpoint: String,
        /// Skip confirmation prompt if tunnel already exists.
        #[clap(long, default_value = "false")]
        yes: bool,
    },

    /// Update an existing tunnel.
    Update {
        /// Tunnel ID (resource name).
        #[clap(long)]
        id: String,
        /// New display name for the tunnel.
        #[clap(long)]
        label: Option<String>,
        /// New local address to expose (host:port, e.g. 127.0.0.1:8080).
        #[clap(long)]
        endpoint: Option<String>,
    },

    /// Delete a tunnel.
    Delete {
        /// Tunnel ID (resource name) to delete.
        #[clap(long)]
        id: String,
    },
}

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    // Install the ring-based crypto provider for rustls
    let _ = rustls_ring::default_provider().install_default();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(sentry::integrations::tracing::layer())
        .init();
    if let Ok(path) = dotenv::dotenv() {
        info!("Loaded environment variables from {}", path.display());
    }

    let _sentry_guard = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN")
            .ok()
            .and_then(|s| s.parse().ok()),
        release: sentry::release_name!(),
        send_default_pii: true,
        before_send: Some(std::sync::Arc::new(|event| match event.level {
            sentry::Level::Error | sentry::Level::Fatal => Some(event),
            _ if rand::random::<f64>() < 0.1 => Some(event),
            _ => None,
        })),
        traces_sample_rate: 0.1,
        ..Default::default()
    });

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
        Commands::Auth(args) => {
            let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;
            match args {
                AuthCommands::Status => {
                    if datum.is_authenticated().await? {
                        println!("Authenticated");
                        if let Some(ctx) = datum.selected_context() {
                            println!("  org: {}", ctx.org_id);
                            println!("  project: {}", ctx.project_id);
                        }
                    } else {
                        println!("Not authenticated");
                    }
                }
                AuthCommands::Login => {
                    datum.login().await?;
                    if let Ok(state) = datum.auth_state().get() {
                        println!(
                            "Logged in as {} ({})",
                            state.profile.display_name(),
                            state.profile.email
                        );
                    } else {
                        println!("Login successful");
                    }
                    select_project_interactive(&datum).await?;
                }
                AuthCommands::Logout => {
                    datum.logout().await?;
                    println!("Logged out");
                }
                AuthCommands::List => {
                    let is_auth = datum.is_authenticated().await?;
                    if is_auth {
                        println!("Current user (active):");
                        if let Some(ctx) = datum.selected_context() {
                            println!("  org: {}", ctx.org_id);
                            println!("  project: {}", ctx.project_id);
                        }
                    } else {
                        println!("No authenticated users");
                    }
                    println!();
                    println!("Note: Multi-user storage not yet implemented. Use 'auth switch' to log in as a different user.");
                }
                AuthCommands::Switch => {
                    datum.logout().await?;
                    println!("Switching users...");
                    datum.login().await?;
                    if let Ok(state) = datum.auth_state().get() {
                        println!(
                            "Switched to {} ({})",
                            state.profile.display_name(),
                            state.profile.email
                        );
                    } else {
                        println!("Switched to new user");
                    }
                    select_project_interactive(&datum).await?;
                }
            }
        }
        Commands::Projects(args) => {
            let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;
            match args {
                ProjectsCommands::List => {
                    let orgs = datum.orgs_and_projects().await?;
                    let selected = datum.selected_context();
                    for org in &orgs {
                        println!("{} ({})", org.org.display_name, org.org.resource_id);
                        for project in &org.projects {
                            let active = selected
                                .as_ref()
                                .map(|ctx| ctx.project_id == project.resource_id)
                                .unwrap_or(false);
                            let marker = if active { " *" } else { "" };
                            println!("  {} ({}){}", project.display_name, project.resource_id, marker);
                        }
                    }
                }
                ProjectsCommands::Switch => {
                    select_project_interactive(&datum).await?;
                }
            }
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
                (None, Some(port)) => Some((args.bind_addr, port).into()),
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
            let uds_listener = if let Some(uds_path) = &args.uds {
                if uds_path.exists() {
                    std::fs::remove_file(uds_path)?;
                }
                let listener = tokio::net::UnixListener::bind(uds_path)?;
                println!("UDS gateway at {}", uds_path.display());
                Some(listener)
            } else {
                None
            };
            println!("serving on port {bind_addr}");
            tokio::select! {
                res = lib::gateway::bind_and_serve(
                    secret_key,
                    config,
                    bind_addr,
                    metrics_bind_addr,
                    #[cfg(unix)]
                    uds_listener,
                ) => res?,
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
        Commands::Tunnel(TunnelArgs { project, command: args }) => {
            let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;

            if let Some(project_id) = project {
                let orgs = datum.orgs_and_projects().await?;
                let ctx = resolve_project_context(&orgs, &project_id)
                    .ok_or_else(|| n0_error::anyerr!("project '{}' not found", project_id))?;
                datum.set_selected_context(Some(ctx)).await?;
            }

            let project_id = datum
                .selected_context()
                .map(|ctx| ctx.project_id)
                .ok_or_else(|| {
                    n0_error::anyerr!(
                        "no project selected — pass --project <id> or run 'datumctl ctx use --project <id>'"
                    )
                })?;
            let node = ListenNode::new_for_project(repo.clone(), &project_id).await?;
            let service = TunnelService::new(datum.clone(), node.clone());
            let heartbeat = HeartbeatAgent::new(datum.clone(), node.clone());

            match args {
                TunnelCommands::List => {
                    let tunnels = service.list_active().await?;
                    if tunnels.is_empty() {
                        println!("No tunnels found in current project.");
                    } else {
                        for t in tunnels {
                            let status = if t.accepted && t.programmed {
                                "ready"
                            } else if t.accepted {
                                "accepted"
                            } else {
                                "pending"
                            };
                            let enabled = if t.enabled { "enabled" } else { "disabled" };
                            println!("{} [{}] {} -> {}", t.id, status, t.label, t.endpoint);
                            if !t.hostnames.is_empty() {
                                for h in &t.hostnames {
                                    println!("  hostname: {}", h);
                                }
                            }
                            println!("  status: {}, {}", enabled, status);
                        }
                    }
                }
                TunnelCommands::Listen { label, endpoint, yes } => {
                    let endpoint_id = node.endpoint_id();

                    let existing = service.get_active_by_endpoint(&endpoint).await?;
                    let tunnel_id = if let Some(t) = existing {
                        println!("Found existing tunnel for {}:", endpoint);
                        println!("  id: {}", t.id);
                        println!("  label: {}", t.label);
                        println!("  endpoint: {}", t.endpoint);
                        println!();

                        // Only update if an explicit label was given and it differs.
                        if let Some(label) = label.filter(|l| l != &t.label) {
                            if yes {
                                println!("Updating tunnel (--yes specified)");
                            } else {
                                print!("Update tunnel label to '{}'? [y/N] ", label);
                                std::io::Write::flush(&mut std::io::stdout())?;
                                let mut input = String::new();
                                std::io::stdin().read_line(&mut input)?;
                                if !input.trim().eq_ignore_ascii_case("y") {
                                    println!("Aborted.");
                                    return Ok(());
                                }
                            }
                            let updated = service.update_active(&t.id, &label, &endpoint).await?;
                            println!("Updated tunnel:");
                            println!("  id: {}", updated.id);
                            updated.id
                        } else {
                            println!("Tunnel already configured correctly.");
                            t.id
                        }
                    } else {
                        let label = label.unwrap_or_else(|| {
                            let bytes: [u8; 6] = rand::random();
                            hex::encode(bytes)
                        });
                        let tunnel = service.create_active(&label, &endpoint).await?;
                        println!("Created tunnel:");
                        println!("  id: {}", tunnel.id);
                        println!("  label: {}", tunnel.label);
                        tunnel.id
                    };
                    
                    heartbeat.start().await;
                    if let Some(ctx) = datum.selected_context() {
                        heartbeat.register_project(ctx.project_id).await;
                    }

                    service.set_enabled_active(&tunnel_id, true).await?;
                    println!();
                    println!("Your endpoint ID: {}", endpoint_id);
                    println!("Setting up tunnel...");
                    let progress = await_tunnel_progress(&service, &tunnel_id).await?;

                    let elapsed = progress.elapsed.as_secs();
                    for hostname in &progress.hostnames {
                        println!("Tunnel ready after {} sec: https://{}", elapsed, hostname);
                    }
                    println!("Press Ctrl+C to stop...");

                    // Watch login state so a permanent auth loss mid-session
                    // (refresh token expired or revoked at the IdP) surfaces to
                    // the operator immediately, with reconnection guidance —
                    // not just buried in tracing output.
                    let mut login_rx = datum.auth().login_state_watch();
                    let mut last_state = *login_rx.borrow();
                    // Also poll the server-side progress every 10s. Setup-time
                    // checks aren't enough: conditions can flip back to a
                    // terminal failure later (e.g. the iroh DNS controller
                    // re-reconciling and re-emerging a stale owner), and the
                    // data plane silently drops in the meantime. When that
                    // happens, surface it and break out so the operator sees
                    // the same actionable message they'd have seen at setup.
                    let mut runtime_poll = tokio::time::interval(RUNTIME_POLL_INTERVAL);
                    runtime_poll.set_missed_tick_behavior(
                        tokio::time::MissedTickBehavior::Delay,
                    );
                    runtime_poll.tick().await; // consume the immediate first tick
                    loop {
                        tokio::select! {
                            res = tokio::signal::ctrl_c() => {
                                res?;
                                break;
                            }
                            res = login_rx.changed() => {
                                if res.is_err() { break; }
                                let new_state = *login_rx.borrow();
                                if new_state == LoginState::Missing
                                    && last_state != LoginState::Missing
                                {
                                    eprintln!();
                                    eprintln!("================================================================");
                                    eprintln!("  Datum login has expired or been revoked.");
                                    eprintln!("  The tunnel will stop accepting new connections until you");
                                    eprintln!("  log in again. Stop this command (Ctrl+C) and run:");
                                    eprintln!();
                                    eprintln!("      datum-connect login");
                                    eprintln!();
                                    eprintln!("  Then restart the tunnel listener.");
                                    eprintln!("================================================================");
                                    eprintln!();
                                }
                                last_state = new_state;
                            }
                            _ = runtime_poll.tick() => {
                                match service.get_active_progress(&tunnel_id).await {
                                    Ok(Some(progress)) => {
                                        if let Some(fail) = progress.terminal_failure() {
                                            eprintln!();
                                            eprintln!("================================================================");
                                            eprintln!("  Tunnel is no longer reachable from the edge.");
                                            eprintln!();
                                            eprintln!("  {}", format_terminal_failure(fail));
                                            eprintln!("================================================================");
                                            eprintln!();
                                            break;
                                        }
                                    }
                                    Ok(None) => {
                                        eprintln!();
                                        eprintln!("Tunnel {} no longer exists on the server. Stopping.", tunnel_id);
                                        break;
                                    }
                                    Err(err) => {
                                        // Transient query failure (network blip, token mid-refresh,
                                        // etc.) — log and keep going; the next tick will retry.
                                        tracing::warn!("watch: progress poll failed: {err:#}");
                                    }
                                }
                            }
                        }
                    }
                    println!();
                    println!("Disabling tunnel...");
                    service.set_enabled_active(&tunnel_id, false).await?;
                    println!("Tunnel disabled.");
                }
                TunnelCommands::Update { id, label, endpoint } => {
                    let current = service.get_active(&id).await?;
                    let current = current.std_context("Tunnel not found")?;
                    let new_label = label.unwrap_or(current.label);
                    let new_endpoint = endpoint.unwrap_or(current.endpoint);
                    let tunnel = service.update_active(&id, &new_label, &new_endpoint).await?;
                    println!("Updated tunnel {}:", tunnel.id);
                    println!("  label: {}", tunnel.label);
                    println!("  endpoint: {}", tunnel.endpoint);
                    if !tunnel.hostnames.is_empty() {
                        println!("  hostnames:");
                        for h in &tunnel.hostnames {
                            println!("    {}", h);
                        }
                    }
                }
                TunnelCommands::Delete { id } => {
                    service.delete_active(&id).await?;
                    println!("Deleted tunnel {}", id);
                }
            }
        }
    }
    Ok(())
}

/// Prompt the user to select an org and project, then persist it as the active context.
async fn select_project_interactive(datum: &DatumCloudClient) -> n0_error::Result<()> {
    use lib::datum_cloud::OrganizationWithProjects;
    use std::io::{BufRead, Write};

    let orgs = datum.orgs_and_projects().await?;
    if orgs.is_empty() {
        println!("No organizations found. Create a project at https://app.datum.net first.");
        return Ok(());
    }

    // Flatten to (org_ref, project_index) for a simple numbered list.
    let mut entries: Vec<(&OrganizationWithProjects, usize)> = Vec::new();
    for org in &orgs {
        for pi in 0..org.projects.len() {
            entries.push((org, pi));
        }
    }

    if entries.is_empty() {
        println!("No projects found. Create a project at https://app.datum.net first.");
        return Ok(());
    }

    if entries.len() == 1 {
        let (org, pi) = entries[0];
        let project = &org.projects[pi];
        let ctx = SelectedContext {
            org_id: org.org.resource_id.clone(),
            org_name: org.org.display_name.clone(),
            project_id: project.resource_id.clone(),
            project_name: project.display_name.clone(),
            org_type: org.org.r#type.clone(),
        };
        println!("Selected project: {} / {}", ctx.org_name, ctx.project_name);
        datum.set_selected_context(Some(ctx)).await?;
        return Ok(());
    }

    println!("\nSelect a project:");
    for (i, (org, pi)) in entries.iter().enumerate() {
        let project = &org.projects[*pi];
        println!("  [{}] {} / {}", i + 1, org.org.display_name, project.display_name);
    }
    print!("Enter number [1-{}]: ", entries.len());
    std::io::stdout().flush().ok();

    let stdin = std::io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .ok_or_else(|| n0_error::anyerr!("no input"))??;
    let choice: usize = line
        .trim()
        .parse()
        .map_err(|_| n0_error::anyerr!("invalid selection"))?;
    if choice < 1 || choice > entries.len() {
        return Err(n0_error::anyerr!("selection out of range"));
    }

    let (org, pi) = entries[choice - 1];
    let project = &org.projects[pi];
    let ctx = SelectedContext {
        org_id: org.org.resource_id.clone(),
        org_name: org.org.display_name.clone(),
        project_id: project.resource_id.clone(),
        project_name: project.display_name.clone(),
        org_type: org.org.r#type.clone(),
    };
    println!("Selected project: {} / {}", ctx.org_name, ctx.project_name);
    datum.set_selected_context(Some(ctx)).await?;
    Ok(())
}

/// Find a project by ID across all orgs and build a `SelectedContext` for it.
fn resolve_project_context(
    orgs: &[lib::datum_cloud::OrganizationWithProjects],
    project_id: &str,
) -> Option<SelectedContext> {
    for org in orgs {
        if let Some(project) = org.projects.iter().find(|p| p.resource_id == project_id) {
            return Some(SelectedContext {
                org_id: org.org.resource_id.clone(),
                org_name: org.org.display_name.clone(),
                project_id: project.resource_id.clone(),
                project_name: project.display_name.clone(),
                org_type: org.org.r#type.clone(),
            });
        }
    }
    None
}

/// Result of streaming the tunnel-setup progress to stdout. All conditions
/// reached `Ready` (or we bailed before that for a terminal failure).
struct SetupResult {
    elapsed: std::time::Duration,
    hostnames: Vec<String>,
}

/// Stuck threshold: a step that stays pending this long without progressing
/// gets called out with a hint. Picked to cover normal slow paths (TLS cert
/// issuance, edge programming) while still flagging genuine wedges.
const PROGRESS_STUCK_WARN: std::time::Duration = std::time::Duration::from_secs(30);

/// Poll cadence during setup. Fast enough that step transitions feel
/// responsive; the actual server-side reconcile latency dominates
/// wall-clock anyway.
const PROGRESS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

/// Poll cadence during the steady-state watch after setup is done. A live
/// terminal failure (e.g. a stale iroh DNS owner re-emerging on a
/// controller re-reconcile) needs to be surfaced within a minute or so,
/// but we don't need sub-second resolution — the tunnel either works or
/// it doesn't.
const RUNTIME_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Format the user-facing message for a terminal progress failure. Shared
/// between setup-time (bail!) and runtime watch (eprintln!) so the user
/// sees the same diagnosis regardless of when the condition trips.
fn format_terminal_failure(fail: &lib::ProgressStep) -> String {
    let owner = fail
        .message
        .as_deref()
        .unwrap_or("(controller did not provide a message)");
    format!(
        "✗ {}: iroh DNS record is owned by another Connector ({}). \
         Another project on this machine — or a stale Connector that was never \
         cleaned up — claimed the same iroh identity. Remove the listen_key for \
         this project (under <repo>/projects/<project_id>/listen_key with the \
         per-project layout, or the flat <repo>/listen_key otherwise) and rerun, \
         or delete the offending Connector.",
        fail.kind.label(),
        owner,
    )
}

/// Drive a tunnel through its setup conditions, printing a checklist as
/// each one transitions to Ready. Bails fast on terminal failure
/// (e.g. iroh DNS deferred to another project's Connector — waiting won't
/// help, and the operator's message already names the conflict).
async fn await_tunnel_progress(
    service: &TunnelService,
    tunnel_id: &str,
) -> n0_error::Result<SetupResult> {
    use std::collections::HashMap;

    let start = std::time::Instant::now();
    let mut last_status: HashMap<ProgressStepKind, StepStatus> = HashMap::new();
    let mut pending_since: HashMap<ProgressStepKind, std::time::Instant> = HashMap::new();
    let mut warned_stuck: std::collections::HashSet<ProgressStepKind> = Default::default();

    loop {
        let Some(progress) = service.get_active_progress(tunnel_id).await? else {
            n0_error::bail_any!("Tunnel {} not found", tunnel_id);
        };

        for step in &progress.steps {
            let prev = last_status.get(&step.kind).copied();
            if prev != Some(step.status) {
                match step.status {
                    StepStatus::Ready => {
                        println!(
                            "  ✓ {} ({:.1}s)",
                            step.kind.label(),
                            start.elapsed().as_secs_f32()
                        );
                        pending_since.remove(&step.kind);
                    }
                    StepStatus::Pending => {
                        pending_since.entry(step.kind).or_insert_with(std::time::Instant::now);
                    }
                    StepStatus::Unknown => {}
                }
                last_status.insert(step.kind, step.status);
            }

            // Generic stuck warning: if a step has been Pending past the
            // threshold and we haven't already warned, surface the
            // controller's reason/message so the user knows what's stalled.
            if step.status == StepStatus::Pending
                && !warned_stuck.contains(&step.kind)
                && let Some(since) = pending_since.get(&step.kind)
                && since.elapsed() >= PROGRESS_STUCK_WARN
            {
                warned_stuck.insert(step.kind);
                let detail = step
                    .message
                    .as_deref()
                    .or(step.reason.as_deref())
                    .unwrap_or("no detail from controller");
                eprintln!(
                    "  … {} still pending after {}s: {}",
                    step.kind.label(),
                    since.elapsed().as_secs(),
                    detail,
                );
            }
        }

        if let Some(fail) = progress.terminal_failure() {
            n0_error::bail_any!("{}", format_terminal_failure(fail));
        }

        if progress.all_ready() && !progress.hostnames.is_empty() {
            return Ok(SetupResult {
                elapsed: start.elapsed(),
                hostnames: progress.hostnames,
            });
        }

        tokio::time::sleep(PROGRESS_POLL_INTERVAL).await;
    }
}
