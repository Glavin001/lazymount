use crate::config::ServerConfig;
use crate::process::{ManagedProcess, ProcessConfig, ProcessEvent};
use crate::types::{ProcessKind, Remote, Result, TunnelMapping};
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

// ---------------------------------------------------------------------------
// Tunnel provider trait
// ---------------------------------------------------------------------------

/// Handle to a running tunnel server.
pub struct TunnelServerHandle {
    pub process: ManagedProcess,
}

/// Handle to a running tunnel client connection.
pub struct TunnelClientHandle {
    pub process: ManagedProcess,
    pub remote_name: String,
    pub tunnels: Vec<TunnelMapping>,
}

#[async_trait]
pub trait TunnelProvider: Send + Sync {
    /// Start a tunnel server (remote side).
    async fn start_server(
        &self,
        config: &ServerConfig,
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<TunnelServerHandle>;

    /// Connect to a tunnel server and establish reverse tunnels (local side).
    async fn connect(
        &self,
        remote: &Remote,
        tunnels: &[TunnelMapping],
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<TunnelClientHandle>;

    /// Update tunnels on an existing connection. For Chisel this requires
    /// a restart of the client process.
    async fn update_tunnels(
        &self,
        handle: &mut TunnelClientHandle,
        remote: &Remote,
        tunnels: &[TunnelMapping],
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Chisel tunnel provider
// ---------------------------------------------------------------------------

pub struct ChiselTunnelProvider;

#[async_trait]
impl TunnelProvider for ChiselTunnelProvider {
    async fn start_server(
        &self,
        config: &ServerConfig,
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<TunnelServerHandle> {
        crate::config::check_binary("chisel")?;

        let mut args = vec![
            "server".to_string(),
            "--port".to_string(),
            config.server.chisel_port.to_string(),
            "--reverse".to_string(),
        ];

        if let Some(ref auth) = config.server.auth {
            args.push("--auth".to_string());
            args.push(auth.clone());
        }

        let proc_config = ProcessConfig {
            name: "chisel-server".to_string(),
            kind: ProcessKind::ChiselServer,
            program: "chisel".to_string(),
            args,
            restart_on_crash: true,
            max_restart_delay: Duration::from_secs(60),
        };

        let process = ManagedProcess::spawn_monitored(proc_config, event_tx).await?;

        info!(port = config.server.chisel_port, "chisel server started");

        Ok(TunnelServerHandle { process })
    }

    async fn connect(
        &self,
        remote: &Remote,
        tunnels: &[TunnelMapping],
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<TunnelClientHandle> {
        crate::config::check_binary("chisel")?;

        let process =
            spawn_chisel_client(remote, tunnels, event_tx).await?;

        info!(
            remote = %remote.name,
            host = %remote.host,
            port = remote.port,
            tunnel_count = tunnels.len(),
            "chisel client connected"
        );

        Ok(TunnelClientHandle {
            process,
            remote_name: remote.name.clone(),
            tunnels: tunnels.to_vec(),
        })
    }

    async fn update_tunnels(
        &self,
        handle: &mut TunnelClientHandle,
        remote: &Remote,
        tunnels: &[TunnelMapping],
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<()> {
        // Chisel doesn't support adding tunnels dynamically.
        // Kill the old client and start a new one with the updated tunnel list.
        info!(
            remote = %remote.name,
            "restarting chisel client to update tunnels (brief interruption)"
        );

        handle.process.kill().await?;

        let process =
            spawn_chisel_client(remote, tunnels, event_tx).await?;
        handle.process = process;
        handle.tunnels = tunnels.to_vec();

        Ok(())
    }
}

async fn spawn_chisel_client(
    remote: &Remote,
    tunnels: &[TunnelMapping],
    event_tx: mpsc::Sender<ProcessEvent>,
) -> Result<ManagedProcess> {
    // Build the server URL with optional auth
    let url = if let Some(ref auth) = remote.auth {
        format!("http://{}@{}:{}", auth, remote.host, remote.port)
    } else {
        format!("http://{}:{}", remote.host, remote.port)
    };

    let mut args = vec!["client".to_string(), url];

    // Add reverse tunnel specs: R:<remote_port>:localhost:<local_port>
    for t in tunnels {
        args.push(format!("R:{}:localhost:{}", t.remote_port, t.local_port));
    }

    let proc_config = ProcessConfig {
        name: format!("chisel-client-{}", remote.name),
        kind: ProcessKind::ChiselClient,
        program: "chisel".to_string(),
        args,
        restart_on_crash: true,
        max_restart_delay: Duration::from_secs(60),
    };

    ManagedProcess::spawn_monitored(proc_config, event_tx).await
}
