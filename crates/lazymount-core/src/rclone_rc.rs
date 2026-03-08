use crate::types::{LazyMountError, MountStats, Result};
use std::time::Duration;
use tracing::warn;

/// Client for rclone's RC (Remote Control) HTTP API.
#[derive(Debug, Clone)]
pub struct RcloneRcClient {
    base_url: String,
    client: reqwest::Client,
}

impl RcloneRcClient {
    pub fn new(port: u16) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build HTTP client");

        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            client,
        }
    }

    /// Health check via rc/noop.
    pub async fn health_check(&self) -> bool {
        let url = format!("{}/rc/noop", self.base_url);
        match self.client.post(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Get VFS cache statistics.
    pub async fn get_vfs_stats(&self) -> Result<MountStats> {
        let url = format!("{}/vfs/stats", self.base_url);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("vfs/stats request failed: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("vfs/stats parse failed: {e}")))?;

        Ok(MountStats {
            cache_size_bytes: body["diskCache"]["bytesUsed"].as_u64().unwrap_or(0),
            cached_files: body["diskCache"]["uploadsInProgress"]
                .as_u64()
                .unwrap_or(0)
                + body["diskCache"]["uploadsQueued"].as_u64().unwrap_or(0),
            bytes_transferred: 0, // populated from core/stats
            transfer_speed_bytes_per_sec: 0.0,
        })
    }

    /// Get transfer statistics.
    pub async fn get_transfer_stats(&self) -> Result<MountStats> {
        let url = format!("{}/core/stats", self.base_url);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("core/stats request failed: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("core/stats parse failed: {e}")))?;

        Ok(MountStats {
            cache_size_bytes: 0,
            cached_files: 0,
            bytes_transferred: body["bytes"].as_u64().unwrap_or(0),
            transfer_speed_bytes_per_sec: body["speed"].as_f64().unwrap_or(0.0),
        })
    }

    /// Evict cached files.
    pub async fn forget_cache(&self, path: Option<&str>) -> Result<()> {
        let url = format!("{}/vfs/forget", self.base_url);
        let mut req = self.client.post(&url);

        if let Some(p) = path {
            req = req.json(&serde_json::json!({ "dir": p }));
        }

        req.send()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("vfs/forget failed: {e}")))?;

        Ok(())
    }

    /// Unmount via RC API.
    pub async fn unmount(&self) -> Result<()> {
        let url = format!("{}/mount/unmount", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| LazyMountError::RcloneRc(format!("mount/unmount failed: {e}")))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!(response = %text, "RC unmount returned non-success");
        }

        Ok(())
    }
}
