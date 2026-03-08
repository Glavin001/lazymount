use crate::config::{CacheConfig, expand_tilde};
use crate::process::{find_available_port, ManagedProcess, ProcessConfig, ProcessEvent};
use crate::rclone_rc::RcloneRcClient;
use crate::types::{LazyMountError, MountStatus, MountStats, MountedShare, ProcessKind, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Manages rclone mount processes on the server side.
pub struct MountManager {
    mounts: HashMap<String, ActiveMount>,
    mount_base_dir: PathBuf,
    cache_config: CacheConfig,
    cache_base_dir: PathBuf,
    next_rc_port: u16,
    event_tx: mpsc::Sender<ProcessEvent>,
}

struct ActiveMount {
    pub info: MountedShare,
    pub process: ManagedProcess,
    pub rc_client: RcloneRcClient,
}

impl MountManager {
    pub fn new(
        mount_base_dir: &str,
        cache_config: CacheConfig,
        cache_base_dir: PathBuf,
        rc_port_start: u16,
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Self {
        Self {
            mounts: HashMap::new(),
            mount_base_dir: expand_tilde(mount_base_dir),
            cache_config,
            cache_base_dir,
            next_rc_port: rc_port_start,
            event_tx,
        }
    }

    /// Mount a share from a connected client.
    pub async fn mount_share(
        &mut self,
        share_name: &str,
        client_name: &str,
        tunneled_port: u16,
    ) -> Result<MountedShare> {
        let key = format!("{client_name}/{share_name}");
        if self.mounts.contains_key(&key) {
            return Err(LazyMountError::Mount(format!(
                "share '{key}' is already mounted"
            )));
        }

        crate::config::check_binary("rclone")?;

        let rc_port = find_available_port(self.next_rc_port)?;
        self.next_rc_port = rc_port + 1;

        let mount_point = self.mount_base_dir.join(share_name);
        let cache_dir = self.cache_base_dir.join(share_name);

        // Ensure mount point and cache directories exist
        std::fs::create_dir_all(&mount_point)?;
        std::fs::create_dir_all(&cache_dir)?;

        let sftp_remote = format!(
            ":sftp,host=localhost,port={},no_check=true:",
            tunneled_port
        );

        let proc_config = ProcessConfig {
            name: format!("rclone-mount-{share_name}"),
            kind: ProcessKind::RcloneMount,
            program: "rclone".to_string(),
            args: vec![
                "mount".to_string(),
                sftp_remote,
                mount_point.to_string_lossy().to_string(),
                "--vfs-cache-mode".to_string(),
                self.cache_config.vfs_cache_mode.clone(),
                "--vfs-cache-max-age".to_string(),
                self.cache_config.vfs_cache_max_age.clone(),
                "--vfs-cache-max-size".to_string(),
                self.cache_config.vfs_cache_max_size.clone(),
                "--vfs-read-ahead".to_string(),
                self.cache_config.vfs_read_ahead.clone(),
                "--cache-dir".to_string(),
                cache_dir.to_string_lossy().to_string(),
                "--dir-cache-time".to_string(),
                "30s".to_string(),
                "--poll-interval".to_string(),
                "15s".to_string(),
                "--rc".to_string(),
                "--rc-addr".to_string(),
                format!("127.0.0.1:{rc_port}"),
                "--rc-no-auth".to_string(),
            ],
            restart_on_crash: true,
            max_restart_delay: Duration::from_secs(60),
        };

        let process =
            ManagedProcess::spawn_monitored(proc_config, self.event_tx.clone()).await?;

        let rc_client = RcloneRcClient::new(rc_port);

        let mount_info = MountedShare {
            share_name: share_name.to_string(),
            client_name: client_name.to_string(),
            tunneled_port,
            mount_point: mount_point.clone(),
            rc_port,
            status: MountStatus::Mounting,
            stats: None,
        };

        info!(
            share_name,
            client_name,
            mount_point = %mount_point.display(),
            rc_port,
            "mounting share"
        );

        self.mounts.insert(
            key,
            ActiveMount {
                info: mount_info.clone(),
                process,
                rc_client,
            },
        );

        Ok(mount_info)
    }

    /// Unmount a share.
    pub async fn unmount_share(&mut self, share_name: &str, client_name: &str) -> Result<()> {
        let key = format!("{client_name}/{share_name}");
        let mut active = self
            .mounts
            .remove(&key)
            .ok_or_else(|| LazyMountError::Mount(format!("mount '{key}' not found")))?;

        // Try RC API unmount first
        if let Err(e) = active.rc_client.unmount().await {
            warn!(share_name, error = %e, "RC unmount failed, falling back to process kill");
        }

        // Kill the rclone mount process
        active.process.kill().await?;

        // Fallback: try system unmount command
        let mount_path = active.info.mount_point.to_string_lossy().to_string();
        let _ = try_system_unmount(&mount_path).await;

        info!(share_name, client_name, "share unmounted");
        Ok(())
    }

    /// List all mounts.
    pub fn list_mounts(&self) -> Vec<MountedShare> {
        self.mounts.values().map(|a| a.info.clone()).collect()
    }

    /// Get stats for a specific mount via rclone RC API.
    pub async fn get_stats(&self, share_name: &str, client_name: &str) -> Result<MountStats> {
        let key = format!("{client_name}/{share_name}");
        let active = self
            .mounts
            .get(&key)
            .ok_or_else(|| LazyMountError::Mount(format!("mount '{key}' not found")))?;

        active.rc_client.get_vfs_stats().await
    }

    /// Health check all mounts via rclone RC API.
    pub async fn health_check_all(&mut self) -> Vec<(String, bool)> {
        let mut results = Vec::new();
        let keys: Vec<String> = self.mounts.keys().cloned().collect();

        for key in keys {
            if let Some(active) = self.mounts.get_mut(&key) {
                let healthy = active.rc_client.health_check().await;
                if healthy {
                    active.info.status = MountStatus::Mounted;
                } else {
                    active.info.status = MountStatus::Stale {
                        since: std::time::Instant::now(),
                    };
                }
                results.push((key, healthy));
            }
        }

        results
    }

    /// Unmount all shares.
    pub async fn shutdown(&mut self) {
        let keys: Vec<(String, String)> = self
            .mounts
            .values()
            .map(|a| (a.info.share_name.clone(), a.info.client_name.clone()))
            .collect();

        for (share_name, client_name) in keys {
            if let Err(e) = self.unmount_share(&share_name, &client_name).await {
                error!(
                    share_name = %share_name,
                    error = %e,
                    "failed to unmount during shutdown"
                );
            }
        }
    }
}

/// Try system unmount command as a fallback.
async fn try_system_unmount(mount_path: &str) -> Result<()> {
    let cmd = if cfg!(target_os = "linux") {
        "fusermount"
    } else {
        "umount"
    };

    let args = if cfg!(target_os = "linux") {
        vec!["-u", mount_path]
    } else {
        vec![mount_path]
    };

    let status = tokio::process::Command::new(cmd)
        .args(&args)
        .status()
        .await
        .map_err(|e| LazyMountError::Mount(format!("system unmount failed: {e}")))?;

    if !status.success() {
        return Err(LazyMountError::Mount(format!(
            "system unmount returned non-zero: {status}"
        )));
    }
    Ok(())
}
