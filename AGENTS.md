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

### End-to-end testing with Docker

The `demo/` directory contains a full E2E test setup that runs locally (Mac or Linux) and in CI:

```
Docker container  = "remote server"  → lazymount server start (chisel server + rclone mount)
Host (Mac/Linux)  = "local machine"  → lazymount daemon + connect + share (rclone serve sftp)
```

#### Files

| File | Purpose |
|------|---------|
| `demo/Dockerfile` | Builds the "remote server" container image (chisel + rclone + lazymount, fuse3) |
| `demo/docker-compose.yml` | Runs the container with `--privileged` (FUSE required) and publishes port 8090 |
| `demo/run-demo.sh` | Interactive demo — leaves the pipeline running for manual exploration |
| `demo/e2e-test.sh` | **Automated test** — pass/fail, cleans up after itself |
| `.github/workflows/e2e.yml` | CI job that builds the binary and runs `e2e-test.sh` |
| `.dockerignore` | Excludes `target/` from Docker build context (prevents multi-GB uploads) |

#### Running the automated E2E tests

**Prerequisites on host:** `chisel`, `rclone`, `docker`

```bash
# Build the host-side binary first
cargo build --release -p lazymount-cli

# Run E2E tests (auto-discovers binary in target/release/)
./demo/e2e-test.sh

# Or point at a specific binary
LAZYMOUNT_BIN=/usr/local/bin/lazymount ./demo/e2e-test.sh
```

The test:
1. Creates a temp directory with known test files
2. Starts the Docker server container (builds image on first run)
3. Starts the host daemon, registers/connects/shares
4. Waits for the FUSE mount to appear inside the container
5. Verifies file existence, content, subdirectories
6. Verifies `lazymount mounts` / `lazymount server status` output
7. Adds a new file and confirms it propagates within 45s (`--dir-cache-time 30s`)
8. Cleans up everything (daemon, container, temp files)

#### Key implementation details

- **Host side** (`rclone serve sftp`): uses `--no-auth` — no credentials needed for the SFTP server.
- **Container side** (`rclone mount`): connects to the SFTP server via the chisel reverse tunnel using `--sftp-user anonymous --sftp-pass <rclone-obscured>` and `key_use_agent=false`. `SSH_AUTH_SOCK` is removed from the rclone mount process environment to prevent SSH agent interference.
- **FUSE in Docker**: requires `--privileged` in docker-compose.yml. Ubuntu 22.04 ships `fusermount3`; the Dockerfile symlinks it to `fusermount` (which rclone expects).
- **File propagation timing**: `--dir-cache-time 30s` and `--poll-interval 15s` mean new files appear within ~30s of being written on the host.
- **Cargo build in Docker**: uses `rust:1.85-bookworm` (requires ≥ 1.85 for edition 2024 / getrandom 0.4). The Dockerfile's `COPY . .` + `cargo build --release` uses Docker layer caching, so rebuilds after code changes only recompile changed crates.

#### Interactive demo

```bash
# Default shares ~/Desktop
./demo/run-demo.sh

# Or share a specific folder
SHARE_PATH=~/code/my-project ./demo/run-demo.sh
```

Then inspect the mounted files inside the container:
```bash
docker compose -f demo/docker-compose.yml exec lazymount-server ls /root/LazyMount/myfiles/
docker compose -f demo/docker-compose.yml exec lazymount-server lazymount mounts
```
