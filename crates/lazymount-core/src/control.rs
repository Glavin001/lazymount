use crate::client::ClientState;
use crate::mounts::MountManager;
use crate::protocol::{ControlRequest, ControlResponse};
use crate::types::{LazyMountError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{error, info};

/// Get the path for the client control socket (Unix) or port file (Windows).
pub fn client_socket_path() -> Result<PathBuf> {
    let data = crate::config::data_dir()?;
    if cfg!(unix) {
        Ok(data.join("lazymount.sock"))
    } else {
        Ok(data.join("lazymount.port"))
    }
}

/// Get the path for the server control socket (Unix) or port file (Windows).
pub fn server_socket_path() -> Result<PathBuf> {
    let data = crate::config::data_dir()?;
    if cfg!(unix) {
        Ok(data.join("lazymount-server.sock"))
    } else {
        Ok(data.join("lazymount-server.port"))
    }
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
    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = bind_listener(socket_path).await?;

    info!(path = %socket_path.display(), "control socket listening");

    loop {
        match accept(&listener).await {
            Ok(stream) => {
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
    stream: Stream,
    handler: F,
) -> Result<()>
where
    F: Fn(ControlRequest) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = ControlResponse> + Send,
{
    let (reader, mut writer) = split_stream(stream);
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
    let stream = connect(socket_path).await?;
    let (reader, mut writer) = split_stream(stream);

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

// ===========================================================================
// Platform abstraction: Unix domain sockets vs TCP on Windows
// ===========================================================================

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(unix)]
type Listener = tokio::net::UnixListener;
#[cfg(unix)]
type ReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(unix)]
type WriteHalf = tokio::net::unix::OwnedWriteHalf;

#[cfg(unix)]
async fn bind_listener(socket_path: &PathBuf) -> Result<Listener> {
    // Clean up stale socket
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }
    tokio::net::UnixListener::bind(socket_path)
        .map_err(|e| LazyMountError::Control(format!("failed to bind socket: {e}")))
}

#[cfg(unix)]
async fn accept(listener: &Listener) -> std::result::Result<Stream, std::io::Error> {
    let (stream, _) = listener.accept().await?;
    Ok(stream)
}

#[cfg(unix)]
async fn connect(socket_path: &PathBuf) -> Result<Stream> {
    tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|e| {
            LazyMountError::Control(format!(
                "failed to connect to daemon at {}: {e}. Is the daemon running?",
                socket_path.display()
            ))
        })
}

#[cfg(unix)]
fn split_stream(stream: Stream) -> (ReadHalf, WriteHalf) {
    stream.into_split()
}

// ---------------------------------------------------------------------------
// Windows: use TCP on localhost. A port file stores the ephemeral port number.
// ---------------------------------------------------------------------------

#[cfg(windows)]
type Stream = tokio::net::TcpStream;
#[cfg(windows)]
type Listener = tokio::net::TcpListener;
#[cfg(windows)]
type ReadHalf = tokio::net::tcp::OwnedReadHalf;
#[cfg(windows)]
type WriteHalf = tokio::net::tcp::OwnedWriteHalf;

#[cfg(windows)]
async fn bind_listener(port_file: &PathBuf) -> Result<Listener> {
    // Bind to an ephemeral port on localhost
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| LazyMountError::Control(format!("failed to bind TCP listener: {e}")))?;

    let port = listener
        .local_addr()
        .map_err(|e| LazyMountError::Control(format!("failed to get local addr: {e}")))?
        .port();

    // Write the port number to the port file so clients can find us
    std::fs::write(port_file, port.to_string())
        .map_err(|e| LazyMountError::Control(format!("failed to write port file: {e}")))?;

    Ok(listener)
}

#[cfg(windows)]
async fn accept(listener: &Listener) -> std::result::Result<Stream, std::io::Error> {
    let (stream, _) = listener.accept().await?;
    Ok(stream)
}

#[cfg(windows)]
async fn connect(port_file: &PathBuf) -> Result<Stream> {
    let port_str = std::fs::read_to_string(port_file).map_err(|e| {
        LazyMountError::Control(format!(
            "failed to read port file at {}: {e}. Is the daemon running?",
            port_file.display()
        ))
    })?;
    let port: u16 = port_str.trim().parse().map_err(|e| {
        LazyMountError::Control(format!("invalid port in {}: {e}", port_file.display()))
    })?;

    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| {
            LazyMountError::Control(format!(
                "failed to connect to daemon on port {port}: {e}. Is the daemon running?"
            ))
        })
}

#[cfg(windows)]
fn split_stream(stream: Stream) -> (ReadHalf, WriteHalf) {
    stream.into_split()
}
