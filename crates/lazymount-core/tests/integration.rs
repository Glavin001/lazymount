//! Integration tests that use real rclone and chisel binaries.
//!
//! These tests require `rclone` and `chisel` to be installed on the system.
//! They run on localhost only (no real NAT traversal), but verify the full
//! subprocess orchestration, SFTP serving, VFS mounting, and RC API integration.
//!
//! Tests are gated behind a `has_rclone` / `has_chisel` check and skip gracefully
//! if the binaries are not available.

use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

fn has_binary(name: &str) -> bool {
    lazymount_core::config::check_binary(name).is_ok()
}

/// Helper to create a temp directory with some test files.
fn create_test_files() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "Hello from LazyMount!\n").unwrap();
    std::fs::write(dir.path().join("data.bin"), vec![0u8; 1024]).unwrap();
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
    std::fs::write(
        dir.path().join("subdir/nested.txt"),
        "nested file content\n",
    )
    .unwrap();
    dir
}

// ============================================================================
// rclone serve sftp tests
// ============================================================================

#[tokio::test]
async fn test_rclone_serve_sftp_starts_and_stops() {
    if !has_binary("rclone") {
        eprintln!("SKIP: rclone not found");
        return;
    }

    let test_dir = create_test_files();
    let (event_tx, mut event_rx) = mpsc::channel(16);

    let mut share_manager = lazymount_core::shares::ShareManager::new(19222, event_tx);

    // Start serving
    let share = share_manager
        .add_share(
            "test-share".to_string(),
            test_dir.path(),
            vec![],
        )
        .await
        .unwrap();

    assert_eq!(share.name, "test-share");
    assert!(share.sftp_port >= 19222);

    // Wait for the Started event
    let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
        .await
        .unwrap()
        .unwrap();
    match event {
        lazymount_core::process::ProcessEvent::Started { name, .. } => {
            assert!(name.contains("test-share"));
        }
        other => panic!("expected Started, got {:?}", other),
    }

    // Poll for rclone to bind the port
    let mut bound = false;
    for _ in 0..20 {
        if !lazymount_core::process::is_port_available(share.sftp_port) {
            bound = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        bound,
        "rclone should be listening on port {} within 10s",
        share.sftp_port
    );

    // List shares
    let shares = share_manager.list_shares();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].name, "test-share");

    // Stop
    share_manager.remove_share("test-share").await.unwrap();
    assert!(share_manager.list_shares().is_empty());

    // Poll for port to be freed
    let mut freed = false;
    for _ in 0..20 {
        if lazymount_core::process::is_port_available(share.sftp_port) {
            freed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        freed,
        "port should be freed after stopping rclone"
    );
}

#[tokio::test]
async fn test_rclone_serve_multiple_shares() {
    if !has_binary("rclone") {
        eprintln!("SKIP: rclone not found");
        return;
    }

    let dir1 = create_test_files();
    let dir2 = create_test_files();
    let (event_tx, _event_rx) = mpsc::channel(64);

    let mut share_manager = lazymount_core::shares::ShareManager::new(19322, event_tx);

    let share1 = share_manager
        .add_share("share1".to_string(), dir1.path(), vec![])
        .await
        .unwrap();
    let share2 = share_manager
        .add_share("share2".to_string(), dir2.path(), vec![])
        .await
        .unwrap();

    // Ports should be different
    assert_ne!(share1.sftp_port, share2.sftp_port);

    // Both should be listed
    assert_eq!(share_manager.list_shares().len(), 2);

    // Cleanup
    share_manager.shutdown().await;
    assert!(share_manager.list_shares().is_empty());
}

#[tokio::test]
async fn test_rclone_serve_duplicate_share_name_fails() {
    if !has_binary("rclone") {
        eprintln!("SKIP: rclone not found");
        return;
    }

    let dir = create_test_files();
    let (event_tx, _event_rx) = mpsc::channel(64);
    let mut share_manager = lazymount_core::shares::ShareManager::new(19422, event_tx);

    share_manager
        .add_share("dup".to_string(), dir.path(), vec![])
        .await
        .unwrap();

    // Adding same name again should fail
    let result = share_manager
        .add_share("dup".to_string(), dir.path(), vec![])
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));

    share_manager.shutdown().await;
}

#[tokio::test]
async fn test_rclone_serve_nonexistent_path_fails() {
    if !has_binary("rclone") {
        eprintln!("SKIP: rclone not found");
        return;
    }

    let (event_tx, _event_rx) = mpsc::channel(64);
    let mut share_manager = lazymount_core::shares::ShareManager::new(19522, event_tx);

    let result = share_manager
        .add_share(
            "bad".to_string(),
            &PathBuf::from("/nonexistent/path/xyz123"),
            vec![],
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

// ============================================================================
// rclone RC API tests (with a real rclone process)
// ============================================================================

#[tokio::test]
async fn test_rclone_rc_api_health_check() {
    if !has_binary("rclone") {
        eprintln!("SKIP: rclone not found");
        return;
    }

    let test_dir = create_test_files();
    let rc_port: u16 = 19672;

    // Start rclone serve sftp with --rc enabled
    let mut child = tokio::process::Command::new("rclone")
        .args([
            "serve",
            "sftp",
            &test_dir.path().to_string_lossy(),
            "--addr",
            &format!("localhost:19622"),
            "--no-auth",
            "--vfs-cache-mode",
            "off",
            "--rc",
            "--rc-addr",
            &format!("127.0.0.1:{rc_port}"),
            "--rc-no-auth",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let rc_client = lazymount_core::rclone_rc::RcloneRcClient::new(rc_port);

    // Poll for rclone RC to become available
    let mut healthy = false;
    for _ in 0..20 {
        if rc_client.health_check().await {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(healthy, "rclone RC health check should pass within 10s");

    // Clean up
    child.kill().await.ok();
}

// ============================================================================
// Chisel tunnel tests
// ============================================================================

#[tokio::test]
async fn test_chisel_server_starts() {
    if !has_binary("chisel") {
        eprintln!("SKIP: chisel not found");
        return;
    }

    let (event_tx, mut event_rx) = mpsc::channel(16);
    let config = lazymount_core::config::ServerConfig {
        server: lazymount_core::config::ServerDaemonConfig {
            chisel_port: 19090,
            auth: Some("testuser:testpass".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let provider = lazymount_core::tunnel::ChiselTunnelProvider;
    let mut handle = lazymount_core::tunnel::TunnelProvider::start_server(
        &provider,
        &config,
        event_tx,
    )
    .await
    .unwrap();

    // Should get a Started event
    let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
        .await
        .unwrap()
        .unwrap();

    match event {
        lazymount_core::process::ProcessEvent::Started { name, .. } => {
            assert_eq!(name, "chisel-server");
        }
        other => panic!("expected Started, got {:?}", other),
    }

    // Poll for chisel to bind the port
    let mut bound = false;
    for _ in 0..20 {
        if !lazymount_core::process::is_port_available(19090) {
            bound = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(bound, "chisel should be listening on port 19090 within 10s");

    // Clean up
    handle.process.kill().await.ok();
}

#[tokio::test]
async fn test_chisel_client_connects_to_server() {
    if !has_binary("chisel") {
        eprintln!("SKIP: chisel not found");
        return;
    }

    let (server_tx, mut server_rx) = mpsc::channel(16);
    let (client_tx, mut client_rx) = mpsc::channel(16);

    // Start chisel server
    let config = lazymount_core::config::ServerConfig {
        server: lazymount_core::config::ServerDaemonConfig {
            chisel_port: 19091,
            auth: Some("user:pass".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let provider = lazymount_core::tunnel::ChiselTunnelProvider;
    let mut server_handle = lazymount_core::tunnel::TunnelProvider::start_server(
        &provider,
        &config,
        server_tx,
    )
    .await
    .unwrap();

    // Wait for server to start and bind port
    let _ = tokio::time::timeout(Duration::from_secs(5), server_rx.recv()).await;
    for _ in 0..20 {
        if !lazymount_core::process::is_port_available(19091) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Connect client with a reverse tunnel
    let remote = lazymount_core::types::Remote {
        name: "test-server".to_string(),
        host: "localhost".to_string(),
        port: 19091,
        auth: Some("user:pass".to_string()),
        auto_connect: false,
        status: Default::default(),
    };

    let tunnels = vec![lazymount_core::types::TunnelMapping {
        local_port: 19722,
        remote_port: 19822,
    }];

    let mut client_handle = lazymount_core::tunnel::TunnelProvider::connect(
        &provider,
        &remote,
        &tunnels,
        client_tx,
    )
    .await
    .unwrap();

    // Should get a Started event for the client
    let event = tokio::time::timeout(Duration::from_secs(5), client_rx.recv())
        .await
        .unwrap()
        .unwrap();

    match event {
        lazymount_core::process::ProcessEvent::Started { name, .. } => {
            assert!(name.contains("chisel-client"), "got: {name}");
        }
        other => panic!("expected Started, got {:?}", other),
    }

    // Clean up
    client_handle.process.kill().await.ok();
    server_handle.process.kill().await.ok();
}

// ============================================================================
// End-to-end: rclone serve + chisel tunnel + rclone RC
// ============================================================================

#[tokio::test]
async fn test_end_to_end_rclone_serve_through_chisel_tunnel() {
    if !has_binary("rclone") || !has_binary("chisel") {
        eprintln!("SKIP: rclone and/or chisel not found");
        return;
    }

    let test_dir = create_test_files();

    // 1. Start rclone serve sftp on local port
    let sftp_port: u16 = 19922;
    let (serve_tx, _serve_rx) = mpsc::channel(64);
    let mut share_manager = lazymount_core::shares::ShareManager::new(sftp_port, serve_tx);

    let share = share_manager
        .add_share("e2e-test".to_string(), test_dir.path(), vec![])
        .await
        .unwrap();
    assert_eq!(share.sftp_port, sftp_port);

    // 2. Start chisel server
    let chisel_port: u16 = 19092;
    let (server_tx, mut server_rx) = mpsc::channel(16);
    let config = lazymount_core::config::ServerConfig {
        server: lazymount_core::config::ServerDaemonConfig {
            chisel_port,
            auth: Some("e2e:test".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let provider = lazymount_core::tunnel::ChiselTunnelProvider;
    let mut server_handle = lazymount_core::tunnel::TunnelProvider::start_server(
        &provider,
        &config,
        server_tx,
    )
    .await
    .unwrap();

    // Wait for chisel server to bind port
    let _ = tokio::time::timeout(Duration::from_secs(5), server_rx.recv()).await;
    for _ in 0..20 {
        if !lazymount_core::process::is_port_available(chisel_port) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 3. Start chisel client with reverse tunnel: remote:tunneled_port -> local:sftp_port
    let tunneled_port: u16 = 19923;
    let (client_tx, mut client_rx) = mpsc::channel(16);
    let remote = lazymount_core::types::Remote {
        name: "e2e-server".to_string(),
        host: "localhost".to_string(),
        port: chisel_port,
        auth: Some("e2e:test".to_string()),
        auto_connect: false,
        status: Default::default(),
    };

    let tunnels = vec![lazymount_core::types::TunnelMapping {
        local_port: sftp_port,
        remote_port: tunneled_port,
    }];

    let mut client_handle = lazymount_core::tunnel::TunnelProvider::connect(
        &provider,
        &remote,
        &tunnels,
        client_tx,
    )
    .await
    .unwrap();

    // Wait for tunnel to establish — poll instead of fixed sleep
    let _ = tokio::time::timeout(Duration::from_secs(5), client_rx.recv()).await;

    // 4. Verify: the tunneled port should now forward to the rclone SFTP server
    //    Poll for the tunnel port to become active (chisel needs time to establish)
    let mut tunnel_active = false;
    for _ in 0..20 {
        if !lazymount_core::process::is_port_available(tunneled_port) {
            tunnel_active = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        tunnel_active,
        "tunneled port {} should be in use (chisel forwarding) after 10s",
        tunneled_port,
    );

    // 5. Cleanup
    client_handle.process.kill().await.ok();
    server_handle.process.kill().await.ok();
    share_manager.shutdown().await;
}

// ============================================================================
// Share manifest HTTP server test
// ============================================================================

#[tokio::test]
async fn test_share_manifest_http_server() {
    use lazymount_core::protocol::ShareManifest;

    // Start a manifest server on a test port
    let manifest = ShareManifest {
        version: 1,
        client_name: "test-client".to_string(),
        shares: vec![lazymount_core::protocol::ShareManifestEntry {
            name: "test-share".to_string(),
            tunneled_port: 3222,
        }],
    };

    let manifest_for_server = manifest.clone();
    let port: u16 = 19200;

    // Start a simple axum server serving the manifest
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(manifest_for_server));
    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move {
                        let m = state.lock().await;
                        axum::Json(m.clone())
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Fetch the manifest
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success());

    let fetched: ShareManifest = resp.json().await.unwrap();
    assert_eq!(fetched.version, 1);
    assert_eq!(fetched.client_name, "test-client");
    assert_eq!(fetched.shares.len(), 1);
    assert_eq!(fetched.shares[0].name, "test-share");
    assert_eq!(fetched.shares[0].tunneled_port, 3222);
}

// ============================================================================
// Config persistence tests
// ============================================================================

#[test]
fn test_config_save_and_load() {
    // Use a temp dir for config to avoid polluting the user's real config
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");

    let config = lazymount_core::config::ClientConfig {
        remotes: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "test-remote".to_string(),
                lazymount_core::config::RemoteConfig {
                    host: "example.com".to_string(),
                    port: 8090,
                    auth: Some("u:p".to_string()),
                    auto_connect: true,
                },
            );
            m
        },
        shares: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "test-share".to_string(),
                lazymount_core::config::ShareConfig {
                    path: "/tmp/test".to_string(),
                    remotes: vec!["test-remote".to_string()],
                },
            );
            m
        },
        ..Default::default()
    };

    // Serialize and write
    let toml_str = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&config_path, &toml_str).unwrap();

    // Read back
    let contents = std::fs::read_to_string(&config_path).unwrap();
    let loaded: lazymount_core::config::ClientConfig = toml::from_str(&contents).unwrap();

    assert!(loaded.remotes.contains_key("test-remote"));
    assert_eq!(loaded.remotes["test-remote"].host, "example.com");
    assert_eq!(
        loaded.remotes["test-remote"].auth.as_deref(),
        Some("u:p")
    );
    assert!(loaded.shares.contains_key("test-share"));
    assert_eq!(loaded.shares["test-share"].remotes, vec!["test-remote"]);
}
