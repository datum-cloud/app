//! Command line arguments.
use clap::{Parser, Subcommand};
use lib::{Node, Repo, TcpProxyTicket};
use std::{
    net::{SocketAddrV4, SocketAddrV6},
    path::PathBuf,
    str::FromStr,
};

/// Datum Connect Agent
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Listen on a magicsocket and forward incoming connections to the specified
    /// host and port.
    Listen(ListenArgs),

    /// Connect to a magicsocket, open a bidi stream, and forward stdin/stdout
    /// to it.
    Connect(ConnectArgs),

    /// Spin up a connect server that will construct connections based
    Serve(ServeArgs),
}

#[derive(Parser, Debug)]
pub struct ListenArgs {
    #[clap(flatten)]
    pub common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct ConnectArgs {
    /// The addresses to listen on for incoming tcp connections.
    ///
    /// To listen on all network interfaces, use 0.0.0.0:12345
    #[clap(long)]
    pub addr: String,

    /// three-word-name for a tunnel to connect to.
    #[clap(long)]
    pub codename: Option<String>,

    /// provide a ticket to drive connection directly.
    #[clap(long)]
    pub ticket: Option<String>,

    #[clap(flatten)]
    pub common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct CommonArgs {
    /// The IPv4 address that magicsocket will listen on.
    ///
    /// If None, defaults to a random free port, but it can be useful to specify a fixed
    /// port, e.g. to configure a firewall rule.
    #[clap(long, default_value = None)]
    pub magic_ipv4_addr: Option<SocketAddrV4>,

    /// The IPv6 address that magicsocket will listen on.
    ///
    /// If None, defaults to a random free port, but it can be useful to specify a fixed
    /// port, e.g. to configure a firewall rule.
    #[clap(long, default_value = None)]
    pub magic_ipv6_addr: Option<SocketAddrV6>,

    /// The verbosity level. Repeat to increase verbosity.
    #[clap(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// The port that magicsocket will listen on.
    ///
    /// If None, defaults to a random free port, but it can be useful to specify a fixed
    /// port, e.g. to configure a firewall rule.
    #[clap(long, default_value = None)]
    pub port: Option<u16>,
}

const REPO_LOCATION_ENV_VAR_NAME: &'static str = "DATUM_CONNECT_PATH";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let path = match std::env::var(REPO_LOCATION_ENV_VAR_NAME) {
        Ok(path) => PathBuf::from(path),
        Err(_) => Repo::default_location(),
    };
    let repo = Repo::open_or_create(path).await?;

    let args = Args::parse();
    match args.command {
        Commands::Listen(_args) => {
            let listen_key = repo.listen_key().await?;
            let node = Node::new(listen_key, repo).await?;
            node.start_listening("".to_string(), "127.0.0.1:5173".to_string())
                .await
                .unwrap();
            println!("{}", node.endpoint_id());
            tokio::signal::ctrl_c().await?;
            println!()
        }
        Commands::Connect(args) => {
            let ConnectArgs {
                addr,
                codename,
                ticket,
                ..
            } = args;
            let connect_key = repo.connect_key().await?;
            let node = Node::new(connect_key, repo).await?;
            let ticket = ticket.map(|s| TcpProxyTicket::from_str(&s).unwrap());

            let _conn = node
                .wrap_connection_tcp(codename, ticket, &addr)
                .await
                .unwrap();
            println!("{}", node.endpoint_id());
            tokio::signal::ctrl_c().await?;
            println!()
        }
        Commands::Serve(args) => {
            let port = args.port.unwrap_or(8080);
            let listen_key = repo.listen_key().await?;
            let node = Node::new(listen_key, repo).await?;
            println!("serving on port {port}");
            lib::http_server::serve(node, port).await?;
            tokio::signal::ctrl_c().await?;
        }
    }
    Ok(())
}
