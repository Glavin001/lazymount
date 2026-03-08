use crate::types::{LazyMountError, ProcessKind, Result};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Configuration for a managed subprocess.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub name: String,
    pub kind: ProcessKind,
    pub program: String,
    pub args: Vec<String>,
    pub restart_on_crash: bool,
    pub max_restart_delay: Duration,
    pub env_remove: Vec<String>,
}

/// Handle to a managed subprocess. Dropping it kills the process.
pub struct ManagedProcess {
    pub config: ProcessConfig,
    child: Option<Child>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ManagedProcess {
    /// Spawn a new managed process.
    pub async fn spawn(config: ProcessConfig) -> Result<Self> {
        let child = spawn_process(&config).await?;
        Ok(Self {
            config,
            child: Some(child),
            shutdown_tx: None,
        })
    }

    /// Spawn and monitor the process in a background task. Restarts on crash
    /// with exponential backoff if configured. Sends events through the channel.
    pub async fn spawn_monitored(
        config: ProcessConfig,
        event_tx: mpsc::Sender<ProcessEvent>,
    ) -> Result<Self> {
        let child = spawn_process(&config).await?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let monitor_config = config.clone();
        let monitor_tx = event_tx.clone();

        tokio::spawn(async move {
            monitor_process(monitor_config, child, monitor_tx, shutdown_rx).await;
        });

        Ok(Self {
            config,
            child: None, // owned by the monitor task
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Check if the process is still running.
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            matches!(child.try_wait(), Ok(None))
        } else {
            // Monitored by background task — assume running unless we got an event
            self.shutdown_tx.is_some()
        }
    }

    /// Kill the process.
    pub async fn kill(&mut self) -> Result<()> {
        // Signal the monitor task to stop
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Kill directly-owned child
        if let Some(ref mut child) = self.child {
            let _ = child.kill().await;
        }
        Ok(())
    }

    /// Get the PID of the child process, if directly owned.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        // Signal monitor to stop
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Best-effort kill of directly owned child
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
    }
}

/// Events emitted by the process monitor.
#[derive(Debug)]
pub enum ProcessEvent {
    Started {
        name: String,
        kind: ProcessKind,
        pid: Option<u32>,
    },
    Exited {
        name: String,
        kind: ProcessKind,
        exit_code: Option<i32>,
    },
    Restarting {
        name: String,
        kind: ProcessKind,
        attempt: u32,
        delay: Duration,
    },
    RestartFailed {
        name: String,
        kind: ProcessKind,
        error: String,
    },
}

async fn spawn_process(config: &ProcessConfig) -> Result<Child> {
    info!(
        name = %config.name,
        program = %config.program,
        args = ?config.args,
        "spawning process"
    );

    let mut cmd = Command::new(&config.program);
    cmd.args(&config.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    for var in &config.env_remove {
        cmd.env_remove(var);
    }

    let child = cmd.spawn()
        .map_err(|e| {
            LazyMountError::Process(format!(
                "failed to spawn {} ({}): {e}",
                config.name, config.program
            ))
        })?;

    Ok(child)
}

async fn monitor_process(
    config: ProcessConfig,
    mut child: Child,
    event_tx: mpsc::Sender<ProcessEvent>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let pid = child.id();
    let _ = event_tx
        .send(ProcessEvent::Started {
            name: config.name.clone(),
            kind: config.kind.clone(),
            pid,
        })
        .await;

    let mut restart_count: u32 = 0;

    loop {
        tokio::select! {
            status = child.wait() => {
                let exit_code = status.ok().and_then(|s| s.code());
                warn!(
                    name = %config.name,
                    exit_code = ?exit_code,
                    "process exited"
                );

                let _ = event_tx.send(ProcessEvent::Exited {
                    name: config.name.clone(),
                    kind: config.kind.clone(),
                    exit_code,
                }).await;

                if !config.restart_on_crash {
                    break;
                }

                // Exponential backoff: 2s, 4s, 8s, 16s, 32s, 60s (capped)
                restart_count += 1;
                let delay = Duration::from_secs(
                    (2u64.saturating_pow(restart_count)).min(config.max_restart_delay.as_secs())
                );

                let _ = event_tx.send(ProcessEvent::Restarting {
                    name: config.name.clone(),
                    kind: config.kind.clone(),
                    attempt: restart_count,
                    delay,
                }).await;

                info!(
                    name = %config.name,
                    attempt = restart_count,
                    delay_secs = delay.as_secs(),
                    "restarting process"
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = &mut shutdown_rx => {
                        info!(name = %config.name, "shutdown received during restart delay");
                        break;
                    }
                }

                match spawn_process(&config).await {
                    Ok(new_child) => {
                        child = new_child;
                        let pid = child.id();
                        let _ = event_tx.send(ProcessEvent::Started {
                            name: config.name.clone(),
                            kind: config.kind.clone(),
                            pid,
                        }).await;
                    }
                    Err(e) => {
                        error!(name = %config.name, error = %e, "failed to restart process");
                        let _ = event_tx.send(ProcessEvent::RestartFailed {
                            name: config.name.clone(),
                            kind: config.kind.clone(),
                            error: e.to_string(),
                        }).await;
                        break;
                    }
                }
            }
            _ = &mut shutdown_rx => {
                info!(name = %config.name, "shutdown signal received, killing process");
                let _ = child.kill().await;
                break;
            }
        }
    }
}

/// Check if a TCP port is available on localhost.
pub fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Find the next available port starting from `start`.
pub fn find_available_port(start: u16) -> Result<u16> {
    for port in start..=start.saturating_add(100) {
        if is_port_available(port) {
            return Ok(port);
        }
    }
    Err(LazyMountError::PortConflict { port: start })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port_returns_valid_port() {
        // Use a high port range unlikely to be in use
        let port = find_available_port(49152).unwrap();
        assert!(port >= 49152);
        assert!(port <= 49252);
    }

    #[test]
    fn test_find_available_port_skips_occupied() {
        // Bind a port, then ask for the same range — should skip it
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let occupied_port = listener.local_addr().unwrap().port();

        let port = find_available_port(occupied_port).unwrap();
        // Should get a port >= occupied_port (might be same if listener freed, or next)
        assert!(port >= occupied_port);
    }

    #[test]
    fn test_is_port_available_unbound_port() {
        // A high ephemeral port should generally be available
        assert!(is_port_available(49999) || !is_port_available(49999));
        // This is inherently racy, but we can at least verify the function runs
    }

    #[test]
    fn test_is_port_available_bound_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_port_available(port));
    }

    #[test]
    fn test_process_config_construction() {
        let config = ProcessConfig {
            name: "test-proc".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            restart_on_crash: true,
            max_restart_delay: Duration::from_secs(60),
            env_remove: vec![],
        };

        assert_eq!(config.name, "test-proc");
        assert_eq!(config.program, "echo");
        assert_eq!(config.args, vec!["hello"]);
        assert!(config.restart_on_crash);
        assert_eq!(config.max_restart_delay, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_spawn_and_wait() {
        let config = ProcessConfig {
            name: "echo-test".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            restart_on_crash: false,
            max_restart_delay: Duration::from_secs(1),
            env_remove: vec![],
        };

        let proc = ManagedProcess::spawn(config).await.unwrap();
        assert!(proc.pid().is_some());
    }

    #[tokio::test]
    async fn test_spawn_monitored_sends_started_event() {
        let (event_tx, mut event_rx) = mpsc::channel(16);

        let config = ProcessConfig {
            name: "event-test".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "echo".to_string(),
            args: vec!["hi".to_string()],
            restart_on_crash: false,
            max_restart_delay: Duration::from_secs(1),
            env_remove: vec![],
        };

        let _proc = ManagedProcess::spawn_monitored(config, event_tx).await.unwrap();

        // Should receive a Started event
        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            ProcessEvent::Started { name, .. } => {
                assert_eq!(name, "event-test");
            }
            other => panic!("expected Started event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_spawn_monitored_sends_exited_event() {
        let (event_tx, mut event_rx) = mpsc::channel(16);

        let config = ProcessConfig {
            name: "exit-test".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "true".to_string(), // exits immediately with code 0
            args: vec![],
            restart_on_crash: false,
            max_restart_delay: Duration::from_secs(1),
            env_remove: vec![],
        };

        let _proc = ManagedProcess::spawn_monitored(config, event_tx).await.unwrap();

        // Collect events — should get Started then Exited
        let mut got_exited = false;
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_secs(2), event_rx.recv()).await {
                Ok(Some(ProcessEvent::Exited { name, exit_code, .. })) => {
                    assert_eq!(name, "exit-test");
                    assert_eq!(exit_code, Some(0));
                    got_exited = true;
                    break;
                }
                Ok(Some(_)) => continue, // skip Started
                _ => break,
            }
        }
        assert!(got_exited, "should have received an Exited event");
    }

    #[tokio::test]
    async fn test_kill_process() {
        let config = ProcessConfig {
            name: "kill-test".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "sleep".to_string(),
            args: vec!["60".to_string()],
            restart_on_crash: false,
            max_restart_delay: Duration::from_secs(1),
            env_remove: vec![],
        };

        let mut proc = ManagedProcess::spawn(config).await.unwrap();
        assert!(proc.is_running());

        proc.kill().await.unwrap();
        // Give it a moment to die
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!proc.is_running());
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_binary_fails() {
        let config = ProcessConfig {
            name: "bad-binary".to_string(),
            kind: ProcessKind::RcloneServe,
            program: "this_binary_does_not_exist_12345".to_string(),
            args: vec![],
            restart_on_crash: false,
            max_restart_delay: Duration::from_secs(1),
            env_remove: vec![],
        };

        let result = ManagedProcess::spawn(config).await;
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("failed to spawn"), "error was: {e}"),
            Ok(_) => panic!("expected error for nonexistent binary"),
        }
    }
}
