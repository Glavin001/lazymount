use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Share manifest — sent from client to server through the control channel
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareManifest {
    pub version: u32,
    pub client_name: String,
    pub shares: Vec<ShareManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareManifestEntry {
    pub name: String,
    pub tunneled_port: u16,
}

// ---------------------------------------------------------------------------
// Control socket IPC protocol — CLI ↔ daemon communication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlRequest {
    // Common
    Status,
    Shutdown,

    // Client-side commands
    RemoteAdd {
        name: String,
        host: String,
        port: u16,
        auth: Option<String>,
    },
    RemoteRemove {
        name: String,
    },
    RemoteList,
    Connect {
        remote: String,
    },
    Disconnect {
        remote: String,
    },
    ShareAdd {
        name: String,
        path: String,
        remote: Option<String>,
    },
    ShareRemove {
        name: String,
    },
    ShareList,

    // Server-side commands
    MountList,
    MountStats {
        share_name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlResponse {
    Ok,
    Error {
        message: String,
    },
    Status {
        role: String,
        running: bool,
        details: String,
    },
    RemoteList {
        remotes: Vec<RemoteInfo>,
    },
    ShareList {
        shares: Vec<ShareInfo>,
    },
    MountList {
        mounts: Vec<MountInfo>,
    },
    MountStats {
        share_name: String,
        cache_size_bytes: u64,
        cached_files: u64,
        bytes_transferred: u64,
        transfer_speed_bytes_per_sec: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInfo {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareInfo {
    pub name: String,
    pub path: String,
    pub remotes: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountInfo {
    pub client_name: String,
    pub share_name: String,
    pub mount_point: String,
    pub cache_size: String,
    pub status: String,
}
