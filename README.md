# LazyMount

Lazy-loading remote file mounts through reverse tunnels. Access local files from any remote server — on-demand, cached, no cloud, no VPN.

## What it does

LazyMount gives you a Dropbox-like experience for sharing local files with remote machines — without syncing files to the cloud. Files are accessed on-demand (lazy-loaded) rather than eagerly synced, so there's no upfront sync cost. Repeated reads on the remote machine are served from a local VFS cache at disk speed.

**Typical use case:** You have project files on your laptop (behind NAT) and remote servers (GPU boxes, cloud VMs) where you need those files. LazyMount makes your local files appear on the remote machines as if they were local.

### How it works

```
Local Machine (behind NAT)              Remote Server (accessible)
┌─────────────────────────┐             ┌─────────────────────────┐
│ rclone serve sftp :2222 │◄──tunnel──► │ rclone mount → ~/LazyMount/
│ chisel client ──────────┼────────────►│ chisel server :8090     │
│                         │             │                         │
│ Files live here         │             │ Files appear here       │
│ ~/code/my-project       │             │ ~/LazyMount/project/    │
└─────────────────────────┘             └─────────────────────────┘
```

1. **Chisel** creates a reverse tunnel from your local machine to the remote server (solves NAT)
2. **rclone serve sftp** exposes your local folders through the tunnel
3. **rclone mount** on the remote mounts those folders with full VFS caching

## Prerequisites

Install these before using LazyMount:

| Software | Install |
|----------|---------|
| **rclone** | https://rclone.org/install/ |
| **chisel** | https://github.com/jpillora/chisel/releases |
| **FUSE** (remote server only) | Linux: `fuse3`, macOS: `macFUSE`, Windows: `WinFsp` |

## Installation

Build from source (requires Rust 1.75+):

```bash
git clone https://github.com/Glavin001/lazymount.git
cd lazymount
cargo build --release
# Binary is at target/release/lazymount
```

## Quick Start

### 1. On your remote server (publicly accessible)

```bash
lazymount server start --auth myuser:mypassword
```

### 2. On your local machine (behind NAT)

```bash
# Start the client daemon
lazymount daemon start

# Register the remote server
lazymount remote add gpu-box gpu.example.com:8090 --auth myuser:mypassword

# Connect to it
lazymount connect gpu-box

# Share a folder
lazymount share add project ~/code/my-project --remote gpu-box
```

### 3. Back on the remote server

Your files appear at `~/LazyMount/project/` — lazy-loaded with local-speed caching after first read.

## CLI Reference

### Server commands (run on remote)

```
lazymount server start [--port 8090] [--auth user:password]
lazymount server stop
lazymount server status
lazymount mounts                    # List mounted shares
lazymount mounts stats <name>       # Cache/transfer stats for a mount
```

### Client commands (run on local)

```
lazymount daemon start              # Start client daemon
lazymount daemon stop               # Stop client daemon

lazymount remote add <name> <host:port> [--auth user:password]
lazymount remote remove <name>
lazymount remote list

lazymount connect <remote>          # Establish tunnel
lazymount disconnect <remote>       # Tear down tunnel

lazymount share add <name> <path> [--remote <remote>]
lazymount share remove <name>
lazymount share list

lazymount status                    # Overall health check
```

## Configuration

### Client config (`~/.config/lazymount/config.toml`)

```toml
[daemon]
sftp_port_range_start = 2222
tunnel_port_range_start = 3222
control_port = 3200

[cache]
vfs_cache_mode = "full"
vfs_cache_max_age = "1h"
vfs_cache_max_size = "10G"
vfs_read_ahead = "128M"

[remotes.gpu-box]
host = "gpu.example.com"
port = 8090
auth = "user:password"
auto_connect = true

[shares.project]
path = "/home/me/code/my-project"
remotes = ["gpu-box"]
```

### Server config (`~/.config/lazymount/server.toml`)

```toml
[server]
chisel_port = 8090
auth = "user:password"
mount_base_dir = "~/LazyMount"
offline_grace_period = 30

[cache]
vfs_cache_mode = "full"
vfs_cache_max_age = "1h"
vfs_cache_max_size = "10G"
vfs_read_ahead = "128M"
```

## Architecture

LazyMount orchestrates two proven open-source tools:

- **[Chisel](https://github.com/jpillora/chisel)** — TCP tunnels over HTTP/WebSocket. Solves the NAT problem by establishing reverse tunnels from your local machine to the remote server.
- **[rclone](https://rclone.org/)** — File serving (`rclone serve sftp`), mounting (`rclone mount`), and VFS caching. First read goes through the tunnel; subsequent reads hit local cache at disk speed.

LazyMount's value is the opinionated orchestration layer: process lifecycle management, automatic tunnel setup, share discovery, cache configuration, and a simple CLI.

### Key design decisions

- **Lazy-loading, not eager sync** — Only files you access are transferred. A 50GB folder doesn't mean 50GB copied.
- **No cloud** — Files stay on your machines. No data stored centrally.
- **NAT-friendly** — Only needs outbound HTTP from your local machine. No port forwarding, no VPN.
- **Cached** — VFS cache means repeated reads are at local disk speed.

## Security

- The chisel server port is the only publicly exposed surface, protected by authentication.
- rclone SFTP servers bind to `localhost` only — not reachable from the network.
- For production use, put chisel behind a TLS reverse proxy (Caddy, nginx) or use chisel's built-in TLS flags (`--tls-key`, `--tls-cert`).

## License

MIT
