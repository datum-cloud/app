//! Command line arguments.
use clap::{Parser, Subcommand};
use lib::{
    Advertisment, AdvertismentTicket, ConnectNode, ListenNode, ProxyState, Repo, TcpProxyData,
    datum_cloud::{ApiEnv, DatumCloudClient},
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
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

#[derive(Parser, Debug)]
pub struct ConnectArgs {
    /// The addresses to listen on for incoming tcp connections.
    ///
    /// If unset uses the addr provided in the advertisment.
    ///
    /// To listen on all network interfaces, use 0.0.0.0:12345
    #[clap(long)]
    pub bind: SocketAddr,

    /// three-word-name for a tunnel to connect to.
    #[clap(long, conflicts_with = "ticket")]
    pub codename: Option<String>,

    /// provide a ticket to drive connection directly.
    #[clap(long, conflicts_with = "codename")]
    pub ticket: Option<AdvertismentTicket>,
}

#[derive(Parser, Debug)]
pub struct ServeArgs {
    #[clap(long, default_value = "0.0.0.0")]
    pub bind_addr: IpAddr,
    #[clap(long, default_value = "8080")]
    pub port: u16,
}

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    tracing_subscriber::fmt::init();
    if let Some(path) = dotenv::dotenv().ok() {
        info!("Loaded environment variables from {}", path.display());
    }

    let args = Args::parse();

    let path = args.repo.unwrap_or_else(Repo::default_location);
    let repo = Repo::open_or_create(path).await?;

    match args.command {
        Commands::List => {
            let datum = DatumCloudClient::with_repo(ApiEnv::Staging, repo.clone()).await?;
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

            println!("");
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
            println!("listening as {}", node.endpoint_id());
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
            let ConnectArgs {
                bind,
                codename,
                ticket,
            } = args;
            let node = ConnectNode::new(repo).await?;
            let ticket = if let Some(codename) = codename {
                node.tickets.get(&codename).await?
            } else if let Some(ticket) = ticket {
                ticket
            } else {
                n0_error::bail_any!("either --codename or --ticket is required");
            };

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
            let secret_key = repo.gateway_key().await?;
            let config = Default::default();
            println!("serving on port {bind_addr}");
            tokio::select! {
                res = lib::gateway::bind_and_serve(secret_key, config, bind_addr) => res?,
                _ = tokio::signal::ctrl_c() => {}
            }
        }
    }
    Ok(())
}
