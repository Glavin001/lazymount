use crate::client::ClientState;
use crate::mounts::MountManager;
use crate::protocol::{ControlRequest, ControlResponse};
use crate::types::{LazyMountError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{error, info};

/// Get the path for the client control socket.
pub fn client_socket_path() -> Result<PathBuf> {
    let data = crate::config::data_dir()?;
    Ok(data.join("lazymount.sock"))
}

/// Get the path for the server control socket.
pub fn server_socket_path() -> Result<PathBuf> {
    let data = crate::config::data_dir()?;
    Ok(data.join("lazymount-server.sock"))
}

/// Serve the control socket for the client daemon.
pub async fn serve_client_control(state: Arc<Mutex<ClientState>>) -> Result<()> {
    let socket_path = client_socket_path()?;
    serve_control_socket(&socket_path, move |request| {
        let state = state.clone();
        async move { crate::client::handle_client_request(request, &state).await }
    })
    .await
}

/// Serve the control socket for the server daemon.
pub async fn serve_server_control(mount_manager: Arc<Mutex<MountManager>>) -> Result<()> {
    let socket_path = server_socket_path()?;
    serve_control_socket(&socket_path, move |request| {
        let mm = mount_manager.clone();
        async move { crate::server::handle_server_request(request, &mm).await }
    })
    .await
}

/// Generic control socket server.
async fn serve_control_socket<F, Fut>(socket_path: &PathBuf, handler: F) -> Result<()>
where
    F: Fn(ControlRequest) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = ControlResponse> + Send,
{
    // Clean up stale socket
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = tokio::net::UnixListener::bind(socket_path)
        .map_err(|e| LazyMountError::Control(format!("failed to bind socket: {e}")))?;

    info!(path = %socket_path.display(), "control socket listening");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let handler = handler.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, handler).await {
                        error!(error = %e, "control socket connection error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "control socket accept error");
            }
        }
    }
}

async fn handle_connection<F, Fut>(
    stream: tokio::net::UnixStream,
    handler: F,
) -> Result<()>
where
    F: Fn(ControlRequest) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = ControlResponse> + Send,
{
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let request: ControlRequest = serde_json::from_str(trimmed)
            .map_err(|e| LazyMountError::Control(format!("invalid request: {e}")))?;

        let response = handler(request).await;

        let response_json = serde_json::to_string(&response)
            .map_err(|e| LazyMountError::Control(format!("failed to serialize response: {e}")))?;

        writer
            .write_all(response_json.as_bytes())
            .await
            .map_err(|e| LazyMountError::Control(format!("failed to write response: {e}")))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| LazyMountError::Control(format!("failed to write newline: {e}")))?;
        writer.flush().await?;

        line.clear();
    }

    Ok(())
}

/// Send a request to a control socket and return the response.
pub async fn send_request(
    socket_path: &PathBuf,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|e| {
            LazyMountError::Control(format!(
                "failed to connect to daemon at {}: {e}. Is the daemon running?",
                socket_path.display()
            ))
        })?;

    let (reader, mut writer) = stream.into_split();

    let request_json = serde_json::to_string(request)
        .map_err(|e| LazyMountError::Control(format!("failed to serialize request: {e}")))?;

    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;

    let response: ControlResponse = serde_json::from_str(response_line.trim())
        .map_err(|e| LazyMountError::Control(format!("invalid response: {e}")))?;

    Ok(response)
}
