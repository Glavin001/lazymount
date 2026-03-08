# AGENTS.md

## Cursor Cloud specific instructions

**Project:** LazyMount — a Rust CLI tool for lazy-loading remote file mounts via reverse tunnels. Two workspace crates: `lazymount-core` (library) and `lazymount-cli` (binary).

### Build / Lint / Test

Standard commands from CI (`.github/workflows/ci.yml`):

- **Build:** `cargo build --all-targets`
- **Lint:** `cargo clippy --all-targets -- -D warnings`
- **Test:** `SSH_AUTH_SOCK="" cargo test -- --test-threads=1`
- **Run CLI:** `cargo run --bin lazymount -- <subcommand>`

### Non-obvious caveats

- Tests **must** run single-threaded (`--test-threads=1`) because several integration tests bind specific TCP ports and will conflict otherwise.
- Set `SSH_AUTH_SOCK=""` when running tests to avoid interference from the host SSH agent (matches CI).
- The integration test `test_end_to_end_rclone_serve_through_chisel_tunnel` is `#[ignore]` due to flakiness; run it manually with `cargo test -- --ignored` if needed.
- External runtime dependencies (`rclone`, `chisel`) must be on `$PATH` for integration tests. Unit tests pass without them but integration tests will skip.
- `libssl-dev` and `pkg-config` are needed at build time (OpenSSL sys crate).
- Rust stable ≥ 1.85 is required (transitive dependency `getrandom` 0.4 uses edition 2024). The VM default may be older; run `rustup default stable && rustup update stable` if builds fail on edition2024.

### Known bugs (as of rclone 1.73.2 / chisel 1.11.4)

- **Chisel auth in URL**: `tunnel.rs` embeds auth in the URL (`http://user:pass@host:port`) but chisel 1.11.4 rejects this as "malformed ws or wss URL." Fix: use `--auth user:pass` as a separate CLI flag.
- **rclone mount SSH agent**: `mounts.rs` constructs the rclone mount command without `--sftp-key-use-agent=false`, `--sftp-user`, or `--sftp-pass`. rclone 1.73.2 always attempts SSH agent connection, causing the mount to fail. The integration tests work around this correctly (see `test_rclone_serve_and_client_e2e`).

### End-to-end testing with Docker

A realistic E2E test can be done using Docker as the "remote server":
1. Host runs `rclone serve sftp` + `chisel client` (local machine side)
2. Docker container runs `chisel server` (remote server side)
3. Container accesses host files via `rclone mount` through the chisel reverse tunnel
4. See the integration test `test_end_to_end_rclone_serve_through_chisel_tunnel` for the pattern; the Docker image needs `rclone`, `chisel`, and `fuse3`
5. Set `SSH_AUTH_SOCK=""` in the container environment, and use `--sftp-key-use-agent=false --sftp-user=anonymous --sftp-pass=<obscured>` when calling rclone as a client
