use crate::types::{LazyMountError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub remotes: HashMap<String, RemoteConfig>,
    #[serde(default)]
    pub shares: HashMap<String, ShareConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_sftp_port_start")]
    pub sftp_port_range_start: u16,
    #[serde(default = "default_tunnel_port_start")]
    pub tunnel_port_range_start: u16,
    #[serde(default = "default_control_port")]
    pub control_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_vfs_cache_mode")]
    pub vfs_cache_mode: String,
    #[serde(default = "default_vfs_cache_max_age")]
    pub vfs_cache_max_age: String,
    #[serde(default = "default_vfs_cache_max_size")]
    pub vfs_cache_max_size: String,
    #[serde(default = "default_vfs_read_ahead")]
    pub vfs_read_ahead: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub host: String,
    #[serde(default = "default_chisel_port")]
    pub port: u16,
    pub auth: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareConfig {
    pub path: String,
    #[serde(default)]
    pub remotes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Server configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub server: ServerDaemonConfig,
    #[serde(default)]
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerDaemonConfig {
    #[serde(default = "default_chisel_port")]
    pub chisel_port: u16,
    pub auth: Option<String>,
    #[serde(default = "default_mount_base_dir")]
    pub mount_base_dir: String,
    #[serde(default = "default_rc_port_start")]
    pub rc_port_range_start: u16,
    #[serde(default = "default_grace_period")]
    pub offline_grace_period: u64,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_sftp_port_start() -> u16 {
    2222
}
fn default_tunnel_port_start() -> u16 {
    3222
}
fn default_control_port() -> u16 {
    3200
}
fn default_chisel_port() -> u16 {
    8090
}
fn default_rc_port_start() -> u16 {
    5572
}
fn default_grace_period() -> u64 {
    30
}
fn default_mount_base_dir() -> String {
    "~/LazyMount".to_string()
}
fn default_vfs_cache_mode() -> String {
    "full".to_string()
}
fn default_vfs_cache_max_age() -> String {
    "1h".to_string()
}
fn default_vfs_cache_max_size() -> String {
    "10G".to_string()
}
fn default_vfs_read_ahead() -> String {
    "128M".to_string()
}

// ---------------------------------------------------------------------------
// Default trait impls
// ---------------------------------------------------------------------------

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            sftp_port_range_start: default_sftp_port_start(),
            tunnel_port_range_start: default_tunnel_port_start(),
            control_port: default_control_port(),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            vfs_cache_mode: default_vfs_cache_mode(),
            vfs_cache_max_age: default_vfs_cache_max_age(),
            vfs_cache_max_size: default_vfs_cache_max_size(),
            vfs_read_ahead: default_vfs_read_ahead(),
        }
    }
}

impl Default for ServerDaemonConfig {
    fn default() -> Self {
        Self {
            chisel_port: default_chisel_port(),
            auth: None,
            mount_base_dir: default_mount_base_dir(),
            rc_port_range_start: default_rc_port_start(),
            offline_grace_period: default_grace_period(),
        }
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Expand `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    } else if path == "~" {
        if let Some(home) = dirs_home() {
            return home;
        }
    }
    PathBuf::from(path)
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Return the lazymount config directory (~/.config/lazymount/).
pub fn config_dir() -> Result<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "lazymount")
        .ok_or_else(|| LazyMountError::Config("cannot determine config directory".into()))?;
    Ok(dir.config_dir().to_path_buf())
}

/// Return the lazymount data directory (~/.local/share/lazymount/).
pub fn data_dir() -> Result<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "lazymount")
        .ok_or_else(|| LazyMountError::Config("cannot determine data directory".into()))?;
    Ok(dir.data_dir().to_path_buf())
}

/// Return the lazymount cache directory (~/.cache/lazymount/).
pub fn cache_dir() -> Result<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "lazymount")
        .ok_or_else(|| LazyMountError::Config("cannot determine cache directory".into()))?;
    Ok(dir.cache_dir().to_path_buf())
}

// ---------------------------------------------------------------------------
// Load / save
// ---------------------------------------------------------------------------

pub fn client_config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn server_config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("server.toml"))
}

pub fn load_client_config() -> Result<ClientConfig> {
    let path = client_config_path()?;
    if !path.exists() {
        return Ok(ClientConfig::default());
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| LazyMountError::Config(format!("failed to read {}: {e}", path.display())))?;
    toml::from_str(&contents)
        .map_err(|e| LazyMountError::Config(format!("failed to parse {}: {e}", path.display())))
}

pub fn save_client_config(config: &ClientConfig) -> Result<()> {
    let path = client_config_path()?;
    ensure_parent_dir(&path)?;
    let contents = toml::to_string_pretty(config)
        .map_err(|e| LazyMountError::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(&path, contents)
        .map_err(|e| LazyMountError::Config(format!("failed to write {}: {e}", path.display())))
}

pub fn load_server_config() -> Result<ServerConfig> {
    let path = server_config_path()?;
    if !path.exists() {
        return Ok(ServerConfig::default());
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| LazyMountError::Config(format!("failed to read {}: {e}", path.display())))?;
    toml::from_str(&contents)
        .map_err(|e| LazyMountError::Config(format!("failed to parse {}: {e}", path.display())))
}

pub fn save_server_config(config: &ServerConfig) -> Result<()> {
    let path = server_config_path()?;
    ensure_parent_dir(&path)?;
    let contents = toml::to_string_pretty(config)
        .map_err(|e| LazyMountError::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(&path, contents)
        .map_err(|e| LazyMountError::Config(format!("failed to write {}: {e}", path.display())))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dependency checks
// ---------------------------------------------------------------------------

/// Check that a binary exists on PATH. Returns the full path if found.
pub fn check_binary(name: &str) -> Result<PathBuf> {
    which(name).ok_or_else(|| LazyMountError::DependencyMissing {
        binary: name.to_string(),
        install_hint: match name {
            "chisel" => "Install from https://github.com/jpillora/chisel/releases".to_string(),
            "rclone" => "Install from https://rclone.org/install/".to_string(),
            _ => format!("Install {name} and ensure it is on your PATH"),
        },
    })
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() {
                Some(full)
            } else {
                None
            }
        })
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_client_config_roundtrip() {
        let config = ClientConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: ClientConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.daemon.sftp_port_range_start, 2222);
        assert_eq!(deserialized.daemon.tunnel_port_range_start, 3222);
        assert_eq!(deserialized.daemon.control_port, 3200);
    }

    #[test]
    fn test_default_server_config_roundtrip() {
        let config = ServerConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: ServerConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.server.chisel_port, 8090);
        assert_eq!(deserialized.server.offline_grace_period, 30);
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/foo/bar");
        assert!(expanded.to_string_lossy().ends_with("foo/bar"));
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_client_config_with_remotes() {
        let toml_str = r#"
[daemon]
sftp_port_range_start = 3000

[remotes.gpu-box]
host = "gpu.example.com"
port = 8090
auth = "user:pass"
auto_connect = true

[shares.project]
path = "/home/me/code"
remotes = ["gpu-box"]
"#;
        let config: ClientConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.daemon.sftp_port_range_start, 3000);
        assert!(config.remotes.contains_key("gpu-box"));
        let remote = &config.remotes["gpu-box"];
        assert_eq!(remote.host, "gpu.example.com");
        assert_eq!(remote.auth.as_deref(), Some("user:pass"));
        assert!(config.shares.contains_key("project"));
    }
}
