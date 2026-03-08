use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LazyMountError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("process error: {0}")]
    Process(String),

    #[error("tunnel error: {0}")]
    Tunnel(String),

    #[error("mount error: {0}")]
    Mount(String),

    #[error("rclone RC error: {0}")]
    RcloneRc(String),

    #[error("control socket error: {0}")]
    Control(String),

    #[error("dependency missing: {binary} — {install_hint}")]
    DependencyMissing {
        binary: String,
        install_hint: String,
    },

    #[error("port conflict: port {port} is already in use")]
    PortConflict { port: u16 },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, LazyMountError>;

// ---------------------------------------------------------------------------
// Client-side types
// ---------------------------------------------------------------------------

/// A configured remote server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub auth: Option<String>,
    pub auto_connect: bool,
    #[serde(skip)]
    pub status: ConnectionStatus,
}

#[derive(Debug, Clone, Default)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting {
        since: Instant,
        attempts: u32,
    },
    Error(String),
}

/// A folder being shared from the local machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub name: String,
    pub path: PathBuf,
    pub sftp_port: u16,
    pub remotes: Vec<String>,
    #[serde(skip)]
    pub status: ShareStatus,
}

#[derive(Debug, Clone, Default)]
pub enum ShareStatus {
    #[default]
    Stopped,
    Starting,
    Serving,
    Error(String),
}

// ---------------------------------------------------------------------------
// Server-side types
// ---------------------------------------------------------------------------

/// A connected client and its shares.
#[derive(Debug)]
pub struct ConnectedClient {
    pub name: String,
    pub connected_since: Instant,
    pub shares: Vec<MountedShare>,
}

/// A share from a connected client, mounted locally.
#[derive(Debug, Clone)]
pub struct MountedShare {
    pub share_name: String,
    pub client_name: String,
    pub tunneled_port: u16,
    pub mount_point: PathBuf,
    pub rc_port: u16,
    pub status: MountStatus,
    pub stats: Option<MountStats>,
}

#[derive(Debug, Clone, Default)]
pub enum MountStatus {
    #[default]
    Discovered,
    Mounting,
    Mounted,
    Unmounting,
    Stale { since: Instant },
    Error(String),
}

/// Cache and transfer stats from rclone RC API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MountStats {
    pub cache_size_bytes: u64,
    pub cached_files: u64,
    pub bytes_transferred: u64,
    pub transfer_speed_bytes_per_sec: f64,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ServerEvent {
    ClientConnected(String),
    ClientDisconnected(String),
    SharesUpdated {
        client_name: String,
        shares: Vec<crate::protocol::ShareManifestEntry>,
    },
    MountHealthCheck {
        share_name: String,
        healthy: bool,
    },
    Shutdown,
}

#[derive(Debug)]
pub enum ClientEvent {
    TunnelEstablished(String),
    TunnelLost(String),
    ShareAdded(String),
    ShareRemoved(String),
    ProcessExited {
        kind: ProcessKind,
        name: String,
        exit_code: Option<i32>,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum ProcessKind {
    RcloneServe,
    RcloneMount,
    ChiselServer,
    ChiselClient,
}

// ---------------------------------------------------------------------------
// Tunnel abstraction
// ---------------------------------------------------------------------------

/// Mapping a local port to a remote port through the tunnel.
#[derive(Debug, Clone)]
pub struct TunnelMapping {
    pub local_port: u16,
    pub remote_port: u16,
}

// ---------------------------------------------------------------------------
// Display impls for status types (for CLI output)
// ---------------------------------------------------------------------------

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Reconnecting { attempts, .. } => {
                write!(f, "reconnecting (attempt {attempts})")
            }
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}

impl std::fmt::Display for ShareStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped"),
            Self::Starting => write!(f, "starting"),
            Self::Serving => write!(f, "serving"),
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}

impl std::fmt::Display for MountStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discovered => write!(f, "discovered"),
            Self::Mounting => write!(f, "mounting"),
            Self::Mounted => write!(f, "mounted"),
            Self::Unmounting => write!(f, "unmounting"),
            Self::Stale { .. } => write!(f, "stale"),
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}
