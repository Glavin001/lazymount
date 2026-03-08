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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_manifest_roundtrip() {
        let manifest = ShareManifest {
            version: 1,
            client_name: "work-laptop".to_string(),
            shares: vec![
                ShareManifestEntry {
                    name: "project".to_string(),
                    tunneled_port: 3222,
                },
                ShareManifestEntry {
                    name: "datasets".to_string(),
                    tunneled_port: 3223,
                },
            ],
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ShareManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.client_name, "work-laptop");
        assert_eq!(parsed.shares.len(), 2);
        assert_eq!(parsed.shares[0].name, "project");
        assert_eq!(parsed.shares[0].tunneled_port, 3222);
        assert_eq!(parsed.shares[1].name, "datasets");
        assert_eq!(parsed.shares[1].tunneled_port, 3223);
    }

    #[test]
    fn test_share_manifest_matches_spec_format() {
        // Verify the JSON format matches exactly what the spec defines
        let json = r#"{
            "version": 1,
            "client_name": "work-laptop",
            "shares": [
                {"name": "project", "tunneled_port": 3222},
                {"name": "datasets", "tunneled_port": 3223}
            ]
        }"#;

        let manifest: ShareManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.shares.len(), 2);
    }

    #[test]
    fn test_share_manifest_empty_shares() {
        let manifest = ShareManifest {
            version: 1,
            client_name: "laptop".to_string(),
            shares: vec![],
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ShareManifest = serde_json::from_str(&json).unwrap();
        assert!(parsed.shares.is_empty());
    }

    #[test]
    fn test_control_request_status_roundtrip() {
        let req = ControlRequest::Status;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ControlRequest::Status));
    }

    #[test]
    fn test_control_request_remote_add_roundtrip() {
        let req = ControlRequest::RemoteAdd {
            name: "gpu-box".to_string(),
            host: "gpu.example.com".to_string(),
            port: 8090,
            auth: Some("user:pass".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlRequest::RemoteAdd { name, host, port, auth } => {
                assert_eq!(name, "gpu-box");
                assert_eq!(host, "gpu.example.com");
                assert_eq!(port, 8090);
                assert_eq!(auth.as_deref(), Some("user:pass"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_request_remote_add_no_auth() {
        let req = ControlRequest::RemoteAdd {
            name: "dev".to_string(),
            host: "dev.local".to_string(),
            port: 9090,
            auth: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlRequest::RemoteAdd { auth, .. } => {
                assert!(auth.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_request_share_add_roundtrip() {
        let req = ControlRequest::ShareAdd {
            name: "project".to_string(),
            path: "/home/user/code".to_string(),
            remote: Some("gpu-box".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlRequest::ShareAdd { name, path, remote } => {
                assert_eq!(name, "project");
                assert_eq!(path, "/home/user/code");
                assert_eq!(remote.as_deref(), Some("gpu-box"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_request_share_add_all_remotes() {
        let req = ControlRequest::ShareAdd {
            name: "project".to_string(),
            path: "/home/user/code".to_string(),
            remote: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlRequest::ShareAdd { remote, .. } => {
                assert!(remote.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_ok_roundtrip() {
        let resp = ControlResponse::Ok;
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ControlResponse::Ok));
    }

    #[test]
    fn test_control_response_error_roundtrip() {
        let resp = ControlResponse::Error {
            message: "share not found".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::Error { message } => assert_eq!(message, "share not found"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_status_roundtrip() {
        let resp = ControlResponse::Status {
            role: "client".to_string(),
            running: true,
            details: "2 connections, 3 shares".to_string(),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::Status { role, running, details } => {
                assert_eq!(role, "client");
                assert!(running);
                assert_eq!(details, "2 connections, 3 shares");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_remote_list_roundtrip() {
        let resp = ControlResponse::RemoteList {
            remotes: vec![
                RemoteInfo {
                    name: "gpu-box".to_string(),
                    host: "gpu.example.com".to_string(),
                    port: 8090,
                    status: "connected".to_string(),
                },
            ],
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::RemoteList { remotes } => {
                assert_eq!(remotes.len(), 1);
                assert_eq!(remotes[0].name, "gpu-box");
                assert_eq!(remotes[0].status, "connected");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_share_list_roundtrip() {
        let resp = ControlResponse::ShareList {
            shares: vec![
                ShareInfo {
                    name: "project".to_string(),
                    path: "/home/user/code".to_string(),
                    remotes: vec!["gpu-box".to_string()],
                    status: "serving".to_string(),
                },
            ],
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::ShareList { shares } => {
                assert_eq!(shares.len(), 1);
                assert_eq!(shares[0].name, "project");
                assert_eq!(shares[0].remotes, vec!["gpu-box"]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_mount_list_roundtrip() {
        let resp = ControlResponse::MountList {
            mounts: vec![
                MountInfo {
                    client_name: "work-laptop".to_string(),
                    share_name: "project".to_string(),
                    mount_point: "/home/user/LazyMount/project".to_string(),
                    cache_size: "2.3 GB".to_string(),
                    status: "mounted".to_string(),
                },
            ],
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::MountList { mounts } => {
                assert_eq!(mounts.len(), 1);
                assert_eq!(mounts[0].client_name, "work-laptop");
                assert_eq!(mounts[0].share_name, "project");
                assert_eq!(mounts[0].cache_size, "2.3 GB");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_control_response_mount_stats_roundtrip() {
        let resp = ControlResponse::MountStats {
            share_name: "project".to_string(),
            cache_size_bytes: 1024 * 1024 * 500,
            cached_files: 42,
            bytes_transferred: 1024 * 1024 * 100,
            transfer_speed_bytes_per_sec: 52428800.0,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlResponse::MountStats {
                share_name,
                cache_size_bytes,
                cached_files,
                bytes_transferred,
                transfer_speed_bytes_per_sec,
            } => {
                assert_eq!(share_name, "project");
                assert_eq!(cache_size_bytes, 1024 * 1024 * 500);
                assert_eq!(cached_files, 42);
                assert_eq!(bytes_transferred, 1024 * 1024 * 100);
                assert!((transfer_speed_bytes_per_sec - 52428800.0).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_all_control_request_variants_serialize() {
        // Ensure every variant can be serialized without panicking
        let variants: Vec<ControlRequest> = vec![
            ControlRequest::Status,
            ControlRequest::Shutdown,
            ControlRequest::RemoteAdd {
                name: "x".into(),
                host: "h".into(),
                port: 1,
                auth: None,
            },
            ControlRequest::RemoteRemove { name: "x".into() },
            ControlRequest::RemoteList,
            ControlRequest::Connect { remote: "x".into() },
            ControlRequest::Disconnect { remote: "x".into() },
            ControlRequest::ShareAdd {
                name: "x".into(),
                path: "/p".into(),
                remote: None,
            },
            ControlRequest::ShareRemove { name: "x".into() },
            ControlRequest::ShareList,
            ControlRequest::MountList,
            ControlRequest::MountStats { share_name: "x".into() },
        ];

        for req in variants {
            let json = serde_json::to_string(&req).unwrap();
            let _parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        }
    }
}
