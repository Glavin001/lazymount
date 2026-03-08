use clap::{Parser, Subcommand};
use lazymount_core::config;
use lazymount_core::control;
use lazymount_core::protocol::{ControlRequest, ControlResponse};
use lazymount_core::types::LazyMountError;

#[derive(Parser)]
#[command(name = "lazymount", about = "Lazy-loading remote file mounts through reverse tunnels")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Server-side commands (run on the remote machine)
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },

    /// Client daemon commands
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },

    /// Register and manage remote servers
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },

    /// Connect to a remote server
    Connect {
        /// Remote server name
        remote: String,
    },

    /// Disconnect from a remote server
    Disconnect {
        /// Remote server name
        remote: String,
    },

    /// Share local folders with remote servers
    Share {
        #[command(subcommand)]
        command: ShareCommands,
    },

    /// List mounted remote shares (server-side)
    Mounts {
        #[command(subcommand)]
        command: Option<MountsCommands>,
    },

    /// Show overall status
    Status,
}

#[derive(Subcommand)]
enum ServerCommands {
    /// Start the server daemon
    Start {
        /// Port for the chisel server
        #[arg(long, default_value_t = 8090)]
        port: u16,

        /// Authentication credentials (user:password)
        #[arg(long)]
        auth: Option<String>,
    },

    /// Stop the server daemon
    Stop,

    /// Show server status
    Status,
}

#[derive(Subcommand)]
enum DaemonCommands {
    /// Start the client daemon
    Start {
        /// Run in background
        #[arg(long)]
        background: bool,
    },

    /// Stop the client daemon
    Stop,
}

#[derive(Subcommand)]
enum RemoteCommands {
    /// Add a remote server
    Add {
        /// Name for this remote
        name: String,

        /// Host and port (host:port)
        host_port: String,

        /// Authentication credentials (user:password)
        #[arg(long)]
        auth: Option<String>,
    },

    /// Remove a remote server
    Remove {
        /// Remote name
        name: String,
    },

    /// List configured remotes
    List,
}

#[derive(Subcommand)]
enum ShareCommands {
    /// Share a local folder
    Add {
        /// Name for this share
        name: String,

        /// Path to the local folder
        path: String,

        /// Share with a specific remote only
        #[arg(long)]
        remote: Option<String>,
    },

    /// Stop sharing a folder
    Remove {
        /// Share name
        name: String,
    },

    /// List shared folders
    List,
}

#[derive(Subcommand)]
enum MountsCommands {
    /// Show stats for a specific mount
    Stats {
        /// Share name
        share_name: String,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("lazymount=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), LazyMountError> {
    match cli.command {
        // ----- Server commands -----
        Commands::Server { command } => match command {
            ServerCommands::Start { port, auth } => {
                // Check dependencies
                config::check_binary("chisel")?;
                config::check_binary("rclone")?;

                let mut server_config = config::load_server_config()?;
                server_config.server.chisel_port = port;
                if auth.is_some() {
                    server_config.server.auth = auth;
                }

                println!("Starting LazyMount server on port {port}...");
                let handle = lazymount_core::server::start_server(server_config).await?;

                // Wait for Ctrl+C
                tokio::signal::ctrl_c().await.ok();
                println!("\nShutting down...");
                handle.shutdown().await?;
            }
            ServerCommands::Stop => {
                let socket = control::server_socket_path()?;
                let resp = control::send_request(&socket, &ControlRequest::Shutdown).await?;
                print_response(&resp);
            }
            ServerCommands::Status => {
                let socket = control::server_socket_path()?;
                let resp = control::send_request(&socket, &ControlRequest::Status).await?;
                print_response(&resp);
            }
        },

        // ----- Daemon commands -----
        Commands::Daemon { command } => match command {
            DaemonCommands::Start { background: _ } => {
                // Check dependencies
                config::check_binary("chisel")?;
                config::check_binary("rclone")?;

                let client_config = config::load_client_config()?;
                println!("Starting LazyMount client daemon...");
                let handle = lazymount_core::client::start_client(client_config).await?;

                // Wait for Ctrl+C
                tokio::signal::ctrl_c().await.ok();
                println!("\nShutting down...");
                handle.shutdown().await?;
            }
            DaemonCommands::Stop => {
                let socket = control::client_socket_path()?;
                let resp = control::send_request(&socket, &ControlRequest::Shutdown).await?;
                print_response(&resp);
            }
        },

        // ----- Remote commands -----
        Commands::Remote { command } => match command {
            RemoteCommands::Add {
                name,
                host_port,
                auth,
            } => {
                let (host, port) = parse_host_port(&host_port)?;
                let socket = control::client_socket_path()?;
                let resp = control::send_request(
                    &socket,
                    &ControlRequest::RemoteAdd {
                        name,
                        host,
                        port,
                        auth,
                    },
                )
                .await?;
                print_response(&resp);
            }
            RemoteCommands::Remove { name } => {
                let socket = control::client_socket_path()?;
                let resp =
                    control::send_request(&socket, &ControlRequest::RemoteRemove { name }).await?;
                print_response(&resp);
            }
            RemoteCommands::List => {
                let socket = control::client_socket_path()?;
                let resp = control::send_request(&socket, &ControlRequest::RemoteList).await?;
                print_response(&resp);
            }
        },

        // ----- Connect / Disconnect -----
        Commands::Connect { remote } => {
            let socket = control::client_socket_path()?;
            let resp =
                control::send_request(&socket, &ControlRequest::Connect { remote }).await?;
            print_response(&resp);
        }
        Commands::Disconnect { remote } => {
            let socket = control::client_socket_path()?;
            let resp =
                control::send_request(&socket, &ControlRequest::Disconnect { remote }).await?;
            print_response(&resp);
        }

        // ----- Share commands -----
        Commands::Share { command } => match command {
            ShareCommands::Add {
                name,
                path,
                remote,
            } => {
                let socket = control::client_socket_path()?;
                let resp = control::send_request(
                    &socket,
                    &ControlRequest::ShareAdd {
                        name,
                        path,
                        remote,
                    },
                )
                .await?;
                print_response(&resp);
            }
            ShareCommands::Remove { name } => {
                let socket = control::client_socket_path()?;
                let resp =
                    control::send_request(&socket, &ControlRequest::ShareRemove { name }).await?;
                print_response(&resp);
            }
            ShareCommands::List => {
                let socket = control::client_socket_path()?;
                let resp = control::send_request(&socket, &ControlRequest::ShareList).await?;
                print_response(&resp);
            }
        },

        // ----- Mounts commands -----
        Commands::Mounts { command } => {
            let socket = control::server_socket_path()?;
            match command {
                None => {
                    let resp =
                        control::send_request(&socket, &ControlRequest::MountList).await?;
                    print_response(&resp);
                }
                Some(MountsCommands::Stats { share_name }) => {
                    let resp = control::send_request(
                        &socket,
                        &ControlRequest::MountStats { share_name },
                    )
                    .await?;
                    print_response(&resp);
                }
            }
        }

        // ----- Status -----
        Commands::Status => {
            // Try client socket first, then server socket
            let client_socket = control::client_socket_path()?;
            let server_socket = control::server_socket_path()?;

            let client_status =
                control::send_request(&client_socket, &ControlRequest::Status).await;
            let server_status =
                control::send_request(&server_socket, &ControlRequest::Status).await;

            match client_status {
                Ok(resp) => {
                    println!("Client daemon:");
                    print_response(&resp);
                }
                Err(_) => {
                    println!("Client daemon: not running");
                }
            }

            println!();

            match server_status {
                Ok(resp) => {
                    println!("Server daemon:");
                    print_response(&resp);
                }
                Err(_) => {
                    println!("Server daemon: not running");
                }
            }
        }
    }

    Ok(())
}

fn print_response(resp: &ControlResponse) {
    match resp {
        ControlResponse::Ok => println!("OK"),
        ControlResponse::Error { message } => {
            eprintln!("Error: {message}");
        }
        ControlResponse::Status {
            role,
            running,
            details,
        } => {
            let status = if *running { "running" } else { "stopped" };
            println!("  Role: {role}");
            println!("  Status: {status}");
            println!("  {details}");
        }
        ControlResponse::RemoteList { remotes } => {
            if remotes.is_empty() {
                println!("No remotes configured.");
                return;
            }
            println!(
                "{:<15} {:<30} {}",
                "NAME", "HOST", "STATUS"
            );
            for r in remotes {
                println!(
                    "{:<15} {:<30} {}",
                    r.name,
                    format!("{}:{}", r.host, r.port),
                    r.status
                );
            }
        }
        ControlResponse::ShareList { shares } => {
            if shares.is_empty() {
                println!("No shares configured.");
                return;
            }
            println!(
                "{:<15} {:<30} {:<15} {}",
                "NAME", "PATH", "REMOTES", "STATUS"
            );
            for s in shares {
                let remotes = if s.remotes.is_empty() {
                    "all".to_string()
                } else {
                    s.remotes.join(", ")
                };
                println!(
                    "{:<15} {:<30} {:<15} {}",
                    s.name, s.path, remotes, s.status
                );
            }
        }
        ControlResponse::MountList { mounts } => {
            if mounts.is_empty() {
                println!("No mounts active.");
                return;
            }
            println!(
                "{:<15} {:<15} {:<30} {:<10} {}",
                "CLIENT", "SHARE", "LOCAL PATH", "CACHE", "STATUS"
            );
            for m in mounts {
                println!(
                    "{:<15} {:<15} {:<30} {:<10} {}",
                    m.client_name, m.share_name, m.mount_point, m.cache_size, m.status
                );
            }
        }
        ControlResponse::MountStats {
            share_name,
            cache_size_bytes,
            cached_files,
            bytes_transferred,
            transfer_speed_bytes_per_sec,
        } => {
            println!("Mount stats for '{share_name}':");
            println!("  Cache size: {cache_size_bytes} bytes");
            println!("  Cached files: {cached_files}");
            println!("  Bytes transferred: {bytes_transferred}");
            println!("  Transfer speed: {transfer_speed_bytes_per_sec:.1} B/s");
        }
    }
}

fn parse_host_port(s: &str) -> Result<(String, u16), LazyMountError> {
    if let Some(colon_pos) = s.rfind(':') {
        let host = s[..colon_pos].to_string();
        let port: u16 = s[colon_pos + 1..]
            .parse()
            .map_err(|_| LazyMountError::Config(format!("invalid port in '{s}'")))?;
        Ok((host, port))
    } else {
        // No port specified, use default
        Ok((s.to_string(), 8090))
    }
}
