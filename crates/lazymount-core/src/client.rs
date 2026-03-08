use crate::config::{ClientConfig, RemoteConfig, ShareConfig};
use crate::process::ProcessEvent;
use crate::protocol::{
    ControlRequest, ControlResponse, RemoteInfo, ShareInfo, ShareManifest, ShareManifestEntry,
};
use crate::shares::ShareManager;
use crate::tunnel::{ChiselTunnelProvider, TunnelClientHandle, TunnelProvider};
use crate::types::{LazyMountError, Remote, Result, TunnelMapping};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

/// Handle to the running client daemon.
pub struct ClientHandle {
    shutdown_tx: mpsc::Sender<()>,
}

impl ClientHandle {
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_tx
            .send(())
            .await
            .map_err(|_| LazyMountError::Other("failed to send shutdown signal".into()))
    }
}

/// Client daemon state, shared across tasks.
pub struct ClientState {
    pub config: ClientConfig,
    pub share_manager: ShareManager,
    pub tunnel_handles: HashMap<String, TunnelClientHandle>,
    pub remotes: HashMap<String, Remote>,
    pub manifest: ShareManifest,
    pub event_tx: mpsc::Sender<ProcessEvent>,
}

impl ClientState {
    /// Get the current tunnel mappings for a remote (control port + all shares for that remote).
    fn tunnel_mappings_for_remote(&self, remote_name: &str) -> Vec<TunnelMapping> {
        let mut mappings = Vec::new();

        // Control channel tunnel
        mappings.push(TunnelMapping {
            local_port: self.config.daemon.control_port,
            remote_port: self.config.daemon.control_port, // 3200 → 3200
        });

        // Share tunnels
        for share in self.share_manager.list_shares() {
            if share.remotes.is_empty() || share.remotes.contains(&remote_name.to_string()) {
                let tunnel_port = self.config.daemon.tunnel_port_range_start
                    + (share.sftp_port - self.config.daemon.sftp_port_range_start);
                mappings.push(TunnelMapping {
                    local_port: share.sftp_port,
                    remote_port: tunnel_port,
                });
            }
        }

        mappings
    }
}

/// Start the client daemon.
pub async fn start_client(config: ClientConfig) -> Result<ClientHandle> {
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let (event_tx, mut event_rx) = mpsc::channel::<ProcessEvent>(64);

    let hostname = hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string();

    let state = Arc::new(Mutex::new(ClientState {
        share_manager: ShareManager::new(
            config.daemon.sftp_port_range_start,
            event_tx.clone(),
        ),
        tunnel_handles: HashMap::new(),
        remotes: HashMap::new(),
        manifest: ShareManifest {
            version: 1,
            client_name: hostname,
            shares: Vec::new(),
        },
        config: config.clone(),
        event_tx: event_tx.clone(),
    }));

    // Start manifest HTTP server on control port
    let manifest_state = state.clone();
    let control_port = config.daemon.control_port;
    tokio::spawn(async move {
        if let Err(e) = serve_manifest(manifest_state, control_port).await {
            error!(error = %e, "manifest server error");
        }
    });

    // Start the control socket listener
    let control_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::control::serve_client_control(control_state).await {
            error!(error = %e, "control socket error");
        }
    });

    // Load saved remotes and shares from config
    {
        let mut s = state.lock().await;
        for (name, remote_cfg) in &config.remotes {
            s.remotes.insert(
                name.clone(),
                Remote {
                    name: name.clone(),
                    host: remote_cfg.host.clone(),
                    port: remote_cfg.port,
                    auth: remote_cfg.auth.clone(),
                    auto_connect: remote_cfg.auto_connect,
                    status: Default::default(),
                },
            );
        }
    }

    // Auto-connect to remotes
    {
        let s = state.lock().await;
        let auto_remotes: Vec<Remote> = s
            .remotes
            .values()
            .filter(|r| r.auto_connect)
            .cloned()
            .collect();
        drop(s);

        for remote in auto_remotes {
            if let Err(e) = connect_to_remote(&state, &remote.name).await {
                warn!(remote = %remote.name, error = %e, "auto-connect failed");
            }
        }
    }

    // Load saved shares from config
    {
        let shares_to_add: Vec<(String, String, Vec<String>)> = config
            .shares
            .iter()
            .map(|(name, share_cfg)| {
                (
                    name.clone(),
                    share_cfg.path.clone(),
                    share_cfg.remotes.clone(),
                )
            })
            .collect();

        for (name, path, remotes) in shares_to_add {
            if let Err(e) = add_share(&state, name.clone(), path, remotes).await {
                warn!(share = %name, error = %e, "failed to start saved share");
            }
        }
    }

    info!("client daemon started");

    // Main event loop
    let event_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = event_rx.recv() => {
                    match event {
                        ProcessEvent::Started { name, pid, .. } => {
                            info!(name = %name, pid = ?pid, "process started");
                        }
                        ProcessEvent::Exited { name, exit_code, .. } => {
                            warn!(name = %name, exit_code = ?exit_code, "process exited");
                        }
                        ProcessEvent::Restarting { name, attempt, delay, .. } => {
                            info!(name = %name, attempt, delay_secs = delay.as_secs(), "restarting");
                        }
                        ProcessEvent::RestartFailed { name, error, .. } => {
                            error!(name = %name, error = %error, "restart failed");
                        }
                    }
                }
                Some(()) = shutdown_rx.recv() => {
                    info!("client shutting down");
                    let mut s = event_state.lock().await;
                    s.share_manager.shutdown().await;
                    for (_, mut handle) in s.tunnel_handles.drain() {
                        handle.process.kill().await.ok();
                    }
                    break;
                }
            }
        }
    });

    Ok(ClientHandle { shutdown_tx })
}

/// Serve the share manifest over HTTP on the control port.
async fn serve_manifest(
    state: Arc<Mutex<ClientState>>,
    port: u16,
) -> Result<()> {
    use axum::{Router, routing::get, extract::State as AxumState, Json};

    let app = Router::new()
        .route("/", get(
            |AxumState(state): AxumState<Arc<Mutex<ClientState>>>| async move {
                let s = state.lock().await;
                Json(s.manifest.clone())
            },
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| LazyMountError::Other(format!("failed to bind manifest server: {e}")))?;

    info!(port, "manifest server listening");

    axum::serve(listener, app)
        .await
        .map_err(|e| LazyMountError::Other(format!("manifest server error: {e}")))?;

    Ok(())
}

/// Connect to a remote server.
pub async fn connect_to_remote(
    state: &Arc<Mutex<ClientState>>,
    remote_name: &str,
) -> Result<()> {
    let mut s = state.lock().await;

    let remote = s
        .remotes
        .get(remote_name)
        .ok_or_else(|| LazyMountError::Other(format!("remote '{remote_name}' not found")))?
        .clone();

    if s.tunnel_handles.contains_key(remote_name) {
        return Err(LazyMountError::Other(format!(
            "already connected to '{remote_name}'"
        )));
    }

    let mappings = s.tunnel_mappings_for_remote(remote_name);
    let event_tx = s.event_tx.clone();

    let provider = ChiselTunnelProvider;
    let handle = provider.connect(&remote, &mappings, event_tx).await?;

    s.tunnel_handles.insert(remote_name.to_string(), handle);
    if let Some(r) = s.remotes.get_mut(remote_name) {
        r.status = crate::types::ConnectionStatus::Connected;
    }

    info!(remote = %remote_name, "connected");
    Ok(())
}

/// Disconnect from a remote server.
pub async fn disconnect_from_remote(
    state: &Arc<Mutex<ClientState>>,
    remote_name: &str,
) -> Result<()> {
    let mut s = state.lock().await;

    let mut handle = s
        .tunnel_handles
        .remove(remote_name)
        .ok_or_else(|| {
            LazyMountError::Other(format!("not connected to '{remote_name}'"))
        })?;

    handle.process.kill().await?;

    if let Some(r) = s.remotes.get_mut(remote_name) {
        r.status = crate::types::ConnectionStatus::Disconnected;
    }

    info!(remote = %remote_name, "disconnected");
    Ok(())
}

/// Add a share.
pub async fn add_share(
    state: &Arc<Mutex<ClientState>>,
    name: String,
    path: String,
    remotes: Vec<String>,
) -> Result<()> {
    let mut s = state.lock().await;

    let expanded_path = crate::config::expand_tilde(&path);
    let share = s
        .share_manager
        .add_share(name.clone(), &expanded_path, remotes.clone())
        .await?;

    // Update manifest
    let tunnel_port = s.config.daemon.tunnel_port_range_start
        + (share.sftp_port - s.config.daemon.sftp_port_range_start);
    s.manifest.shares.push(ShareManifestEntry {
        name: name.clone(),
        tunneled_port: tunnel_port,
    });

    // Update tunnels for all connected remotes that need this share
    let connected_remotes: Vec<String> = s.tunnel_handles.keys().cloned().collect();
    let provider = ChiselTunnelProvider;

    for remote_name in &connected_remotes {
        if remotes.is_empty() || remotes.contains(remote_name) {
            let mappings = s.tunnel_mappings_for_remote(remote_name);
            let remote = s.remotes.get(remote_name).cloned();
            let event_tx = s.event_tx.clone();

            if let (Some(remote), Some(handle)) =
                (remote, s.tunnel_handles.get_mut(remote_name))
            {
                if let Err(e) =
                    provider.update_tunnels(handle, &remote, &mappings, event_tx).await
                {
                    warn!(
                        remote = %remote_name,
                        error = %e,
                        "failed to update tunnels"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Remove a share.
pub async fn remove_share(
    state: &Arc<Mutex<ClientState>>,
    name: &str,
) -> Result<()> {
    let mut s = state.lock().await;

    s.share_manager.remove_share(name).await?;

    // Remove from manifest
    s.manifest.shares.retain(|e| e.name != name);

    // Update tunnels for all connected remotes
    let connected_remotes: Vec<String> = s.tunnel_handles.keys().cloned().collect();
    let provider = ChiselTunnelProvider;

    for remote_name in &connected_remotes {
        let mappings = s.tunnel_mappings_for_remote(remote_name);
        let remote = s.remotes.get(remote_name.as_str()).cloned();
        let event_tx = s.event_tx.clone();

        if let (Some(remote), Some(handle)) =
            (remote, s.tunnel_handles.get_mut(remote_name.as_str()))
        {
            if let Err(e) =
                provider.update_tunnels(handle, &remote, &mappings, event_tx).await
            {
                warn!(
                    remote = %remote_name,
                    error = %e,
                    "failed to update tunnels after share removal"
                );
            }
        }
    }

    Ok(())
}

/// Handle a control request on the client side.
pub async fn handle_client_request(
    request: ControlRequest,
    state: &Arc<Mutex<ClientState>>,
) -> ControlResponse {
    match request {
        ControlRequest::Status => {
            let s = state.lock().await;
            let connected = s.tunnel_handles.len();
            let shares = s.share_manager.list_shares().len();
            ControlResponse::Status {
                role: "client".to_string(),
                running: true,
                details: format!(
                    "{connected} connection(s), {shares} share(s)"
                ),
            }
        }
        ControlRequest::RemoteAdd {
            name,
            host,
            port,
            auth,
        } => {
            let mut s = state.lock().await;
            s.remotes.insert(
                name.clone(),
                Remote {
                    name: name.clone(),
                    host,
                    port,
                    auth,
                    auto_connect: false,
                    status: Default::default(),
                },
            );

            // Save to config
            let remote_ref = s.remotes.get(&name).unwrap();
            let remote_config = RemoteConfig {
                host: remote_ref.host.clone(),
                port: remote_ref.port,
                auth: remote_ref.auth.clone(),
                auto_connect: false,
            };
            s.config.remotes.insert(name, remote_config);
            if let Err(e) = crate::config::save_client_config(&s.config) {
                return ControlResponse::Error {
                    message: format!("saved in memory but failed to persist config: {e}"),
                };
            }

            ControlResponse::Ok
        }
        ControlRequest::RemoteRemove { name } => {
            let mut s = state.lock().await;
            if s.remotes.remove(&name).is_none() {
                return ControlResponse::Error {
                    message: format!("remote '{name}' not found"),
                };
            }
            s.config.remotes.remove(&name);
            let _ = crate::config::save_client_config(&s.config);
            ControlResponse::Ok
        }
        ControlRequest::RemoteList => {
            let s = state.lock().await;
            let remotes = s
                .remotes
                .values()
                .map(|r| RemoteInfo {
                    name: r.name.clone(),
                    host: r.host.clone(),
                    port: r.port,
                    status: r.status.to_string(),
                })
                .collect();
            ControlResponse::RemoteList { remotes }
        }
        ControlRequest::Connect { remote } => {
            match connect_to_remote(state, &remote).await {
                Ok(()) => ControlResponse::Ok,
                Err(e) => ControlResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        ControlRequest::Disconnect { remote } => {
            match disconnect_from_remote(state, &remote).await {
                Ok(()) => ControlResponse::Ok,
                Err(e) => ControlResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        ControlRequest::ShareAdd {
            name,
            path,
            remote,
        } => {
            let remotes = match remote {
                Some(r) => vec![r],
                None => Vec::new(),
            };

            // Also persist to config
            {
                let mut s = state.lock().await;
                s.config.shares.insert(
                    name.clone(),
                    ShareConfig {
                        path: path.clone(),
                        remotes: remotes.clone(),
                    },
                );
                let _ = crate::config::save_client_config(&s.config);
            }

            match add_share(state, name, path, remotes).await {
                Ok(()) => ControlResponse::Ok,
                Err(e) => ControlResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        ControlRequest::ShareRemove { name } => {
            {
                let mut s = state.lock().await;
                s.config.shares.remove(&name);
                let _ = crate::config::save_client_config(&s.config);
            }

            match remove_share(state, &name).await {
                Ok(()) => ControlResponse::Ok,
                Err(e) => ControlResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        ControlRequest::ShareList => {
            let s = state.lock().await;
            let shares = s
                .share_manager
                .list_shares()
                .iter()
                .map(|sh| ShareInfo {
                    name: sh.name.clone(),
                    path: sh.path.to_string_lossy().to_string(),
                    remotes: sh.remotes.clone(),
                    status: sh.status.to_string(),
                })
                .collect();
            ControlResponse::ShareList { shares }
        }
        ControlRequest::Shutdown => ControlResponse::Ok,
        _ => ControlResponse::Error {
            message: "command not supported on client".to_string(),
        },
    }
}
