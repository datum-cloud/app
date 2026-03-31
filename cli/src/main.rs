//! Command line arguments.
use clap::{Parser, Subcommand};
mod dns_dev;
mod tunnel_dev;

use lib::{
    Advertisment, AdvertismentTicket, ConnectNode, HeartbeatAgent, ListenNode, ProxyState, Repo,
    SelectedContext, TcpProxyData, TunnelService,
    datum_cloud::{ApiEnv, DatumCloudClient},
};
use n0_error::StdResultExt;
use std::{net::SocketAddr, path::PathBuf};
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

            let node = ListenNode::new(repo.clone()).await?;
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
                    let setup_start = std::time::Instant::now();

                    let tunnel = loop {
                        let t = service.get_active(&tunnel_id).await?;
                        let Some(t) = t else {
                            n0_error::bail_any!("Tunnel {} not found", tunnel_id);
                        };
                        if t.accepted && t.programmed && !t.hostnames.is_empty() {
                            break t;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    };

                    let elapsed = setup_start.elapsed().as_secs();
                    for hostname in &tunnel.hostnames {
                        println!("Tunnel ready after {} sec: https://{}", elapsed, hostname);
                    }
                    println!("Press Ctrl+C to stop...");

                    tokio::signal::ctrl_c().await?;
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
