use crate::config::{cache_dir, ServerConfig};
use crate::mounts::MountManager;
use crate::process::ProcessEvent;
use crate::protocol::{ControlRequest, ControlResponse, MountInfo, ShareManifest};
use crate::tunnel::{ChiselTunnelProvider, TunnelProvider};
use crate::types::{LazyMountError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

/// Handle to the running server daemon.
pub struct ServerHandle {
    shutdown_tx: mpsc::Sender<()>,
}

impl ServerHandle {
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_tx
            .send(())
            .await
            .map_err(|_| LazyMountError::Other("failed to send shutdown signal".into()))
    }
}

/// Start the server daemon.
pub async fn start_server(config: ServerConfig) -> Result<ServerHandle> {
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let (event_tx, mut event_rx) = mpsc::channel::<ProcessEvent>(64);

    let tunnel_provider = ChiselTunnelProvider;
    let cache_base = cache_dir().unwrap_or_else(|_| "/tmp/lazymount-cache".into());

    // Start chisel server
    let mut tunnel_handle = tunnel_provider
        .start_server(&config, event_tx.clone())
        .await?;

    info!(
        port = config.server.chisel_port,
        "server daemon started"
    );

    // Create mount manager
    let mount_manager = Arc::new(Mutex::new(MountManager::new(
        &config.server.mount_base_dir,
        config.cache.clone(),
        cache_base,
        config.server.rc_port_range_start,
        event_tx.clone(),
    )));

    let mm = mount_manager.clone();
    let grace_period = config.server.offline_grace_period;

    // Start the control channel poller (polls client manifest via tunneled port 3200)
    let poll_mm = mount_manager.clone();
    let poll_event_tx = event_tx.clone();
    tokio::spawn(async move {
        poll_client_manifest(poll_mm, poll_event_tx, grace_period).await;
    });

    // Start the control socket listener
    let control_mm = mount_manager.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::control::serve_server_control(control_mm).await {
            error!(error = %e, "control socket error");
        }
    });

    // Main event loop
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
                            info!(
                                name = %name,
                                attempt,
                                delay_secs = delay.as_secs(),
                                "process restarting"
                            );
                        }
                        ProcessEvent::RestartFailed { name, error, .. } => {
                            error!(name = %name, error = %error, "process restart failed");
                        }
                    }
                }
                Some(()) = shutdown_rx.recv() => {
                    info!("server shutting down");
                    mm.lock().await.shutdown().await;
                    tunnel_handle.process.kill().await.ok();
                    break;
                }
            }
        }
    });

    Ok(ServerHandle { shutdown_tx })
}

/// Poll the client's share manifest via the tunneled control channel.
async fn poll_client_manifest(
    mount_manager: Arc<Mutex<MountManager>>,
    _event_tx: mpsc::Sender<ProcessEvent>,
    grace_period: u64,
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let mut last_manifest: Option<ShareManifest> = None;
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        match client
            .get("http://localhost:3200/")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                consecutive_failures = 0;

                match resp.json::<ShareManifest>().await {
                    Ok(manifest) => {
                        let changed = last_manifest
                            .as_ref()
                            .map(|prev| {
                                prev.shares.len() != manifest.shares.len()
                                    || prev.shares.iter().zip(&manifest.shares).any(
                                        |(a, b)| {
                                            a.name != b.name
                                                || a.tunneled_port != b.tunneled_port
                                        },
                                    )
                            })
                            .unwrap_or(true);

                        if changed {
                            info!(
                                client = %manifest.client_name,
                                shares = manifest.shares.len(),
                                "client manifest updated"
                            );

                            reconcile_mounts(
                                &mount_manager,
                                &manifest,
                            )
                            .await;
                        }

                        last_manifest = Some(manifest);
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to parse client manifest");
                    }
                }
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "control channel returned non-success");
                consecutive_failures += 1;
            }
            Err(_) => {
                consecutive_failures += 1;
                if consecutive_failures == 1 {
                    // Only log on first failure to avoid spam
                    info!("waiting for client to connect...");
                }
            }
        }

        // If client is offline for longer than grace period, unmount everything
        if consecutive_failures > 0
            && consecutive_failures * 5 >= grace_period as u32
            && last_manifest.is_some()
        {
            warn!("client offline past grace period, unmounting all shares");
            mount_manager.lock().await.shutdown().await;
            last_manifest = None;
            consecutive_failures = 0;
        }
    }
}

/// Compare the manifest against current mounts and mount/unmount as needed.
async fn reconcile_mounts(
    mount_manager: &Arc<Mutex<MountManager>>,
    manifest: &ShareManifest,
) {
    let mut mm = mount_manager.lock().await;

    let current_mounts = mm.list_mounts();
    let current_names: std::collections::HashSet<String> =
        current_mounts.iter().map(|m| m.share_name.clone()).collect();
    let manifest_names: std::collections::HashSet<String> =
        manifest.shares.iter().map(|s| s.name.clone()).collect();

    // Unmount shares no longer in the manifest
    for name in &current_names {
        if !manifest_names.contains(name) {
            info!(share = %name, "unmounting removed share");
            if let Err(e) = mm.unmount_share(name, &manifest.client_name).await {
                error!(share = %name, error = %e, "failed to unmount removed share");
            }
        }
    }

    // Mount new shares
    for entry in &manifest.shares {
        if !current_names.contains(&entry.name) {
            info!(share = %entry.name, port = entry.tunneled_port, "mounting new share");
            if let Err(e) = mm
                .mount_share(&entry.name, &manifest.client_name, entry.tunneled_port)
                .await
            {
                error!(
                    share = %entry.name,
                    error = %e,
                    "failed to mount share"
                );
            }
        }
    }
}

/// Handle a control request on the server side.
pub async fn handle_server_request(
    request: ControlRequest,
    mount_manager: &Arc<Mutex<MountManager>>,
) -> ControlResponse {
    match request {
        ControlRequest::Status => ControlResponse::Status {
            role: "server".to_string(),
            running: true,
            details: "server daemon running".to_string(),
        },
        ControlRequest::MountList => {
            let mm = mount_manager.lock().await;
            let mounts = mm
                .list_mounts()
                .iter()
                .map(|m| MountInfo {
                    client_name: m.client_name.clone(),
                    share_name: m.share_name.clone(),
                    mount_point: m.mount_point.to_string_lossy().to_string(),
                    cache_size: format_bytes(m.stats.as_ref().map(|s| s.cache_size_bytes).unwrap_or(0)),
                    status: m.status.to_string(),
                })
                .collect();
            ControlResponse::MountList { mounts }
        }
        ControlRequest::MountStats { share_name } => {
            let mm = mount_manager.lock().await;
            // Try to find the mount — look for any client
            let mounts = mm.list_mounts();
            if let Some(mount) = mounts.iter().find(|m| m.share_name == share_name) {
                match mm.get_stats(&share_name, &mount.client_name).await {
                    Ok(stats) => ControlResponse::MountStats {
                        share_name,
                        cache_size_bytes: stats.cache_size_bytes,
                        cached_files: stats.cached_files,
                        bytes_transferred: stats.bytes_transferred,
                        transfer_speed_bytes_per_sec: stats.transfer_speed_bytes_per_sec,
                    },
                    Err(e) => ControlResponse::Error {
                        message: format!("failed to get stats: {e}"),
                    },
                }
            } else {
                ControlResponse::Error {
                    message: format!("mount '{share_name}' not found"),
                }
            }
        }
        ControlRequest::Shutdown => {
            // The actual shutdown is handled by the caller
            ControlResponse::Ok
        }
        _ => ControlResponse::Error {
            message: "command not supported on server".to_string(),
        },
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
