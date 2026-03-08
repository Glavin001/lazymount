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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_status_display() {
        assert_eq!(ConnectionStatus::Disconnected.to_string(), "disconnected");
        assert_eq!(ConnectionStatus::Connecting.to_string(), "connecting");
        assert_eq!(ConnectionStatus::Connected.to_string(), "connected");
        assert_eq!(
            ConnectionStatus::Reconnecting {
                since: Instant::now(),
                attempts: 3
            }
            .to_string(),
            "reconnecting (attempt 3)"
        );
        assert_eq!(
            ConnectionStatus::Error("timeout".to_string()).to_string(),
            "error: timeout"
        );
    }

    #[test]
    fn test_share_status_display() {
        assert_eq!(ShareStatus::Stopped.to_string(), "stopped");
        assert_eq!(ShareStatus::Starting.to_string(), "starting");
        assert_eq!(ShareStatus::Serving.to_string(), "serving");
        assert_eq!(
            ShareStatus::Error("port in use".to_string()).to_string(),
            "error: port in use"
        );
    }

    #[test]
    fn test_mount_status_display() {
        assert_eq!(MountStatus::Discovered.to_string(), "discovered");
        assert_eq!(MountStatus::Mounting.to_string(), "mounting");
        assert_eq!(MountStatus::Mounted.to_string(), "mounted");
        assert_eq!(MountStatus::Unmounting.to_string(), "unmounting");
        assert_eq!(
            MountStatus::Stale {
                since: Instant::now()
            }
            .to_string(),
            "stale"
        );
        assert_eq!(
            MountStatus::Error("fuse error".to_string()).to_string(),
            "error: fuse error"
        );
    }

    #[test]
    fn test_connection_status_default() {
        let status = ConnectionStatus::default();
        assert!(matches!(status, ConnectionStatus::Disconnected));
    }

    #[test]
    fn test_share_status_default() {
        let status = ShareStatus::default();
        assert!(matches!(status, ShareStatus::Stopped));
    }

    #[test]
    fn test_mount_status_default() {
        let status = MountStatus::default();
        assert!(matches!(status, MountStatus::Discovered));
    }

    #[test]
    fn test_error_display() {
        let err = LazyMountError::Config("bad toml".to_string());
        assert_eq!(err.to_string(), "configuration error: bad toml");

        let err = LazyMountError::DependencyMissing {
            binary: "chisel".to_string(),
            install_hint: "install from github".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "dependency missing: chisel — install from github"
        );

        let err = LazyMountError::PortConflict { port: 8090 };
        assert_eq!(err.to_string(), "port conflict: port 8090 is already in use");
    }

    #[test]
    fn test_tunnel_mapping() {
        let mapping = TunnelMapping {
            local_port: 2222,
            remote_port: 3222,
        };
        assert_eq!(mapping.local_port, 2222);
        assert_eq!(mapping.remote_port, 3222);
    }

    #[test]
    fn test_mount_stats_default() {
        let stats = MountStats::default();
        assert_eq!(stats.cache_size_bytes, 0);
        assert_eq!(stats.cached_files, 0);
        assert_eq!(stats.bytes_transferred, 0);
        assert_eq!(stats.transfer_speed_bytes_per_sec, 0.0);
    }

    #[test]
    fn test_mount_stats_serialization() {
        let stats = MountStats {
            cache_size_bytes: 1024,
            cached_files: 5,
            bytes_transferred: 2048,
            transfer_speed_bytes_per_sec: 100.5,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let parsed: MountStats = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.cache_size_bytes, 1024);
        assert_eq!(parsed.cached_files, 5);
        assert_eq!(parsed.bytes_transferred, 2048);
        assert!((parsed.transfer_speed_bytes_per_sec - 100.5).abs() < 0.01);
    }

    #[test]
    fn test_remote_serialization() {
        let remote = Remote {
            name: "gpu-box".to_string(),
            host: "gpu.example.com".to_string(),
            port: 8090,
            auth: Some("user:pass".to_string()),
            auto_connect: true,
            status: ConnectionStatus::Connected,
        };

        let json = serde_json::to_string(&remote).unwrap();
        let parsed: Remote = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "gpu-box");
        assert_eq!(parsed.host, "gpu.example.com");
        assert_eq!(parsed.port, 8090);
        assert_eq!(parsed.auth.as_deref(), Some("user:pass"));
        assert!(parsed.auto_connect);
        // status is #[serde(skip)] so it should be default (Disconnected)
        assert!(matches!(parsed.status, ConnectionStatus::Disconnected));
    }

    #[test]
    fn test_share_serialization() {
        let share = Share {
            name: "project".to_string(),
            path: PathBuf::from("/home/user/code"),
            sftp_port: 2222,
            remotes: vec!["gpu-box".to_string()],
            status: ShareStatus::Serving,
        };

        let json = serde_json::to_string(&share).unwrap();
        let parsed: Share = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "project");
        assert_eq!(parsed.path, PathBuf::from("/home/user/code"));
        assert_eq!(parsed.sftp_port, 2222);
        assert_eq!(parsed.remotes, vec!["gpu-box"]);
        // status is #[serde(skip)] so it should be default (Stopped)
        assert!(matches!(parsed.status, ShareStatus::Stopped));
    }
}
