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

    let child = Command::new(&config.program)
        .args(&config.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
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
