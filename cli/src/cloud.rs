//! Login, context, agent, and tunnel CLI commands.

use std::fs::OpenOptions;
use std::process::Stdio;
use std::time::Duration;

use lib::agent::{
    self, AgentClient, AgentStatus, CreateTunnelRequest, running_agent_info, wait_until_ready,
};
use lib::datum_cloud::{ApiEnv, DatumCloudClient, LoginState, resolve_selected_context};
use lib::{Repo, SelectedContext};
use n0_error::{Result, StdResultExt};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{AgentCommands, ContextCommands, TunnelCommands};

pub async fn login(repo: Repo, force: bool, json: bool) -> Result<()> {
    let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo).await?;
    if force {
        datum.auth().logout().await?;
    }
    datum.auth().login().await?;
    let auth = datum.auth_state();
    let auth = auth.get()?;
    let payload = LoginOutput {
        email: auth.profile.email.clone(),
        user_id: auth.profile.user_id.clone(),
    };
    if json {
        print_json(&payload)?;
    } else {
        println!("Logged in as {}", payload.email);
    }
    Ok(())
}

pub async fn logout(repo: Repo, json: bool) -> Result<()> {
    let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo).await?;
    datum.auth().logout().await?;
    if json {
        print_json(&serde_json::json!({ "logged_in": false }))?;
    } else {
        println!("Logged out.");
    }
    Ok(())
}

pub async fn status(repo: Repo, json: bool) -> Result<()> {
    let payload = collect_status(&repo).await?;
    if json {
        print_json(&payload)?;
        return Ok(());
    }

    match &payload.email {
        Some(email) => println!("Logged in as {email} ({})", payload.login_state),
        None => println!("Not logged in. Run `datum-connect login`."),
    }
    match &payload.context {
        Some(ctx) => println!("Project: {} / {}", ctx.org_name, ctx.project_name),
        None => println!("No project selected. Run `datum-connect context set --project <id>`."),
    }
    match &payload.agent {
        agent if agent.running => {
            println!(
                "Agent: running (pid {}, endpoint {})",
                agent.pid.unwrap_or_default(),
                agent.endpoint_id.as_deref().unwrap_or("-")
            );
        }
        _ => println!("Agent: not running. Run `datum-connect agent start`."),
    }
    Ok(())
}

async fn collect_status(repo: &Repo) -> Result<StatusOutput> {
    let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;
    let auth = datum.auth_state();
    let profile = auth.get().ok();
    let agent = match AgentClient::connect(repo) {
        Ok(client) => match client.status().await {
            Ok(status) => AgentStatusOutput::from_running(status),
            Err(_) => AgentStatusOutput::from_info(running_agent_info(repo)?),
        },
        Err(_) => AgentStatusOutput::from_info(running_agent_info(repo)?),
    };
    Ok(StatusOutput {
        logged_in: profile.is_some(),
        login_state: login_state_name(datum.login_state()).to_string(),
        email: profile.map(|p| p.profile.email.clone()),
        user_id: profile.map(|p| p.profile.user_id.clone()),
        context: datum.selected_context(),
        agent,
    })
}

pub async fn context(repo: Repo, command: ContextCommands) -> Result<()> {
    match command {
        ContextCommands::Show { json } => context_show(repo, json).await,
        ContextCommands::List { json } => context_list(repo, json).await,
        ContextCommands::Set { project, org, json } => context_set(repo, project, org, json).await,
    }
}

async fn context_show(repo: Repo, json: bool) -> Result<()> {
    let datum = require_logged_in(repo).await?;
    let selected = datum.selected_context();
    if json {
        print_json(&ContextShowOutput {
            context: selected.clone(),
        })?;
        return Ok(());
    }
    match selected {
        Some(ctx) => {
            println!("{} / {}", ctx.org_name, ctx.project_name);
            println!("  org:     {}", ctx.org_id);
            println!("  project: {}", ctx.project_id);
        }
        None => println!("No project selected. Run `datum-connect context set --project <id>`."),
    }
    Ok(())
}

async fn context_list(repo: Repo, json: bool) -> Result<()> {
    let datum = require_logged_in(repo).await?;
    let orgs = datum.orgs_and_projects().await?;
    if json {
        print_json(&orgs)?;
        return Ok(());
    }
    if orgs.is_empty() {
        println!("No organizations found.");
        return Ok(());
    }
    let selected = datum.selected_context();
    for org in &orgs {
        println!("{} ({})", org.org.display_name, org.org.resource_id);
        if org.projects.is_empty() {
            println!("  (no projects)");
            continue;
        }
        for project in &org.projects {
            let marker = if selected
                .as_ref()
                .is_some_and(|ctx| ctx.project_id == project.resource_id)
            {
                "*"
            } else {
                " "
            };
            println!(
                " {marker} {} ({})",
                project.display_name, project.resource_id
            );
        }
    }
    Ok(())
}

async fn context_set(repo: Repo, project: String, org: Option<String>, json: bool) -> Result<()> {
    let datum = require_logged_in(repo).await?;
    let orgs = datum.orgs_and_projects().await?;
    let ctx = resolve_selected_context(&orgs, &project, org.as_deref())?;
    datum.set_selected_context(Some(ctx.clone())).await?;
    if json {
        print_json(&ctx)?;
    } else {
        println!("Selected {} / {}", ctx.org_name, ctx.project_name);
    }
    Ok(())
}

pub async fn agent(repo: Repo, command: AgentCommands) -> Result<()> {
    match command {
        AgentCommands::Start => agent_start(repo).await,
        AgentCommands::Stop => {
            agent::stop_agent(&repo).await?;
            println!("Agent stopped.");
            Ok(())
        }
        AgentCommands::Status { json } => {
            let info = running_agent_info(&repo)?;
            if json {
                print_json(&AgentStatusOutput::from_info(info))?;
                return Ok(());
            }
            match info {
                Some(info) => println!(
                    "Agent running (pid {}, endpoint {}, control http://127.0.0.1:{})",
                    info.pid, info.endpoint_id, info.port
                ),
                None => println!("Agent is not running."),
            }
            Ok(())
        }
    }
}

async fn agent_start(repo: Repo) -> Result<()> {
    if let Some(info) = running_agent_info(&repo)? {
        n0_error::bail_any!(
            "Agent is already running (pid {}). Stop it with `datum-connect agent stop`.",
            info.pid
        );
    }

    let shutdown = CancellationToken::new();
    let shutdown_for_signal = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_for_signal.cancel();
    });

    println!("Starting agent…");
    agent::run_agent(repo, shutdown).await?;
    println!("Agent stopped.");
    Ok(())
}

pub async fn tunnel(repo: Repo, command: TunnelCommands) -> Result<()> {
    match command {
        TunnelCommands::Create {
            label,
            endpoint,
            wait_hostname,
            timeout,
            json,
        } => {
            let client = connect_or_start(&repo).await?;
            let tunnel = client
                .create_tunnel(&CreateTunnelRequest {
                    label,
                    endpoint,
                    wait_hostname,
                    timeout_secs: Some(Duration::from(timeout).as_secs()),
                })
                .await?;
            if json {
                print_json(&tunnel)?;
            } else {
                println!("{} -> {}", tunnel.label, tunnel.endpoint);
                match &tunnel.url {
                    Some(url) => println!("{url}"),
                    None => println!("(hostname pending; tunnel id {})", tunnel.id),
                }
            }
            Ok(())
        }
        TunnelCommands::List { json } => {
            let client = AgentClient::connect(&repo)?;
            let tunnels = client.list_tunnels().await?;
            if json {
                print_json(&tunnels)?;
            } else if tunnels.is_empty() {
                println!("No tunnels.");
            } else {
                for tunnel in tunnels {
                    let url = tunnel.url.as_deref().unwrap_or("(hostname pending)");
                    println!(
                        "{}  {}  {} -> {}",
                        tunnel.id, tunnel.label, url, tunnel.endpoint
                    );
                }
            }
            Ok(())
        }
        TunnelCommands::Get { id, json } => {
            let client = AgentClient::connect(&repo)?;
            let tunnel = client.get_tunnel(&id).await?;
            if json {
                print_json(&tunnel)?;
            } else {
                println!("{} ({})", tunnel.label, tunnel.id);
                println!("  endpoint: {}", tunnel.endpoint);
                match &tunnel.url {
                    Some(url) => println!("  url: {url}"),
                    None => println!("  url: (hostname pending)"),
                }
            }
            Ok(())
        }
        TunnelCommands::Delete { id, json } => {
            let client = AgentClient::connect(&repo)?;
            let outcome = client.delete_tunnel(&id).await?;
            if json {
                print_json(&outcome)?;
            } else {
                println!("Deleted tunnel {}", outcome.id);
            }
            Ok(())
        }
    }
}

async fn connect_or_start(repo: &Repo) -> Result<AgentClient> {
    if let Ok(client) = AgentClient::connect(repo) {
        return Ok(client);
    }
    eprintln!("Starting datum-connect agent…");
    spawn_detached_agent(repo)?;
    match wait_until_ready(repo, Duration::from_secs(20)).await {
        Ok(client) => Ok(client),
        Err(err) => n0_error::bail_any!(
            "{err:#}. Check {} for agent logs.",
            repo.agent_log_path().display()
        ),
    }
}

pub fn spawn_detached_agent(repo: &Repo) -> Result<()> {
    let exe = std::env::current_exe().std_context("failed to resolve current executable")?;
    let log_path = repo.agent_log_path();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .std_context("failed to open agent.log")?;
    let log_err = log.try_clone().std_context("failed to clone agent.log")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--repo").arg(repo.path());
    cmd.args(["agent", "start"]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(log));
    cmd.stderr(Stdio::from(log_err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .std_context("failed to spawn datum-connect agent")?;
    Ok(())
}

async fn require_logged_in(repo: Repo) -> Result<DatumCloudClient> {
    let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo).await?;
    if datum.auth_state().get().is_err() {
        n0_error::bail_any!("Not logged in. Run `datum-connect login`.");
    }
    Ok(datum)
}

fn login_state_name(state: LoginState) -> &'static str {
    match state {
        LoginState::Missing => "missing",
        LoginState::NeedsRefresh => "needs_refresh",
        LoginState::Valid => "valid",
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).std_context("serializing json")?
    );
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

#[derive(Serialize)]
struct LoginOutput {
    email: String,
    user_id: String,
}

#[derive(Serialize)]
struct StatusOutput {
    logged_in: bool,
    login_state: String,
    email: Option<String>,
    user_id: Option<String>,
    context: Option<SelectedContext>,
    agent: AgentStatusOutput,
}

#[derive(Serialize)]
struct ContextShowOutput {
    context: Option<SelectedContext>,
}

#[derive(Serialize)]
struct AgentStatusOutput {
    running: bool,
    pid: Option<u32>,
    port: Option<u16>,
    endpoint_id: Option<String>,
}

impl AgentStatusOutput {
    fn from_running(status: AgentStatus) -> Self {
        Self {
            running: true,
            pid: Some(status.pid),
            port: Some(status.port),
            endpoint_id: Some(status.endpoint_id),
        }
    }

    fn from_info(info: Option<agent::AgentInfo>) -> Self {
        match info {
            Some(info) => Self {
                running: true,
                pid: Some(info.pid),
                port: Some(info.port),
                endpoint_id: Some(info.endpoint_id),
            },
            None => Self {
                running: false,
                pid: None,
                port: None,
                endpoint_id: None,
            },
        }
    }
}
