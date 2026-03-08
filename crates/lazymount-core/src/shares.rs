use crate::process::{find_available_port, ManagedProcess, ProcessConfig, ProcessEvent};
use crate::types::{LazyMountError, ProcessKind, Result, Share, ShareStatus};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;

/// Manages rclone serve sftp processes on the client side.
pub struct ShareManager {
    shares: HashMap<String, ActiveShare>,
    next_sftp_port: u16,
    event_tx: mpsc::Sender<ProcessEvent>,
}

struct ActiveShare {
    pub share: Share,
    pub process: ManagedProcess,
}

impl ShareManager {
    pub fn new(sftp_port_start: u16, event_tx: mpsc::Sender<ProcessEvent>) -> Self {
        Self {
            shares: HashMap::new(),
            next_sftp_port: sftp_port_start,
            event_tx,
        }
    }

    /// Start serving a local folder over SFTP.
    pub async fn add_share(
        &mut self,
        name: String,
        path: &Path,
        remotes: Vec<String>,
    ) -> Result<Share> {
        if self.shares.contains_key(&name) {
            return Err(LazyMountError::Other(format!(
                "share '{name}' already exists"
            )));
        }

        if !path.exists() {
            return Err(LazyMountError::Other(format!(
                "path '{}' does not exist",
                path.display()
            )));
        }

        crate::config::check_binary("rclone")?;

        let sftp_port = find_available_port(self.next_sftp_port)?;
        self.next_sftp_port = sftp_port + 1;

        let proc_config = ProcessConfig {
            name: format!("rclone-serve-{name}"),
            kind: ProcessKind::RcloneServe,
            program: "rclone".to_string(),
            args: vec![
                "serve".to_string(),
                "sftp".to_string(),
                path.to_string_lossy().to_string(),
                "--addr".to_string(),
                format!("localhost:{sftp_port}"),
                "--no-auth".to_string(),
                "--vfs-cache-mode".to_string(),
                "off".to_string(),
            ],
            restart_on_crash: true,
            max_restart_delay: Duration::from_secs(60),
        };

        let process =
            ManagedProcess::spawn_monitored(proc_config, self.event_tx.clone()).await?;

        let share = Share {
            name: name.clone(),
            path: path.to_path_buf(),
            sftp_port,
            remotes,
            status: ShareStatus::Serving,
        };

        info!(
            name = %share.name,
            path = %share.path.display(),
            sftp_port,
            "share started"
        );

        self.shares.insert(
            name,
            ActiveShare {
                share: share.clone(),
                process,
            },
        );

        Ok(share)
    }

    /// Stop serving a folder.
    pub async fn remove_share(&mut self, name: &str) -> Result<()> {
        let mut active = self
            .shares
            .remove(name)
            .ok_or_else(|| LazyMountError::Other(format!("share '{name}' not found")))?;

        active.process.kill().await?;
        info!(name, "share stopped");
        Ok(())
    }

    /// List all active shares.
    pub fn list_shares(&self) -> Vec<Share> {
        self.shares
            .values()
            .map(|a| a.share.clone())
            .collect()
    }

    /// Get a share by name.
    pub fn get_share(&self, name: &str) -> Option<&Share> {
        self.shares.get(name).map(|a| &a.share)
    }

    /// Stop all shares.
    pub async fn shutdown(&mut self) {
        let names: Vec<String> = self.shares.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.remove_share(&name).await {
                tracing::error!(name = %name, error = %e, "failed to stop share during shutdown");
            }
        }
    }
}
