# Monitor

Monitor is a small, self-hosted Linux monitoring system: one embedded-web
Server, outbound-only Linux Agents, resource and traffic history, and optional
per-node ICMP latency targets. It is single-admin and intentionally has no
remote shell, command execution, updater daemon, notifications, plugins,
multi-user/RBAC, or Docker management.

## Requirements and build

Production runtime is Linux with systemd. Installation also requires `curl`,
`sha256sum`, and `ss` from iproute2. The Server listens on loopback by default
and the Agent makes outbound HTTP(S) connections. Build the embedded web assets
and Rust binaries from the repository:

```sh
cd web && npm ci && npm run build
cd .. && cargo build --release --workspace
```

The binaries are `target/release/monitor-server` and
`target/release/monitor-agent`; both report `0.1.1` with `--version`.

## Quick install

The bootstrap script resolves the latest release, verifies the release
checksums through `install-monitor.sh`, and prompts for secrets on the
terminal. Secrets are never accepted as command-line arguments, so nothing
sensitive reaches your shell history, `ps`, or the process environment.

Server, listening on `127.0.0.1:25774` by default:

```sh
curl -fsSL https://raw.githubusercontent.com/bear4f/Monitor/main/scripts/bootstrap-monitor.sh \
  | sudo bash -s -- server --set-admin-password
```

A custom port, or a custom address and port:

```sh
curl -fsSL .../bootstrap-monitor.sh | sudo bash -s -- server --port 25776
curl -fsSL .../bootstrap-monitor.sh | sudo bash -s -- server --listen 0.0.0.0 --port 25776
```

`--listen` accepts IPv4 and IPv6 literals; `::1` and `::` are bracketed
automatically. Binding to `0.0.0.0` or `::` prints a warning: Monitor serves
plain HTTP and expects a trusted HTTPS reverse proxy in front of it.

Agent, which prompts for the token with echo disabled:

```sh
curl -fsSL .../bootstrap-monitor.sh \
  | sudo bash -s -- agent --server https://monitor.example.com
```

Pin an exact release instead of the latest with `--version v0.1.1`. For
unattended installs, pass a root-owned `0600` file with
`--admin-password-file PATH` or `--token-file PATH`; the file is read but never
copied or deleted.

Reinstalling preserves a custom listener and never changes an existing
administrator password unless `--set-admin-password` or
`--admin-password-file` is given.

## Server installation (explicit release)

Install an explicit GitHub Release version. The installer downloads the
matching `monitor-server-linux-amd64` or `monitor-server-linux-arm64` asset and
the `SHA256SUMS` asset from that same release, verifies SHA256, then installs
atomically:

```sh
sudo ./scripts/install-monitor.sh --version v0.1.1 --component server
sudo ./scripts/install-monitor.sh --version v0.1.1 --component server \
  --listen 0.0.0.0 --port 25776
```

The unit runs as the dedicated `monitor:monitor` user with no capabilities,
and by default executes:

```text
/usr/local/bin/monitor-server --listen 127.0.0.1:25774 --db /var/lib/monitor/monitor.db
```

The shipped unit file is never rewritten. A non-default listener is applied
through a managed drop-in at
`/etc/systemd/system/monitor-server.service.d/10-monitor-listen.conf`, which
carries a marker line so uninstall removes only that file and leaves any
drop-in you added yourself untouched. A drop-in you own that also sets
`ExecStart` makes the installer refuse rather than guess.

The database and WAL files persist under `/var/lib/monitor`. A first admin
password is set through stdin as the service user; do not put it in a command
argument or log:

```sh
sudo -u monitor /usr/local/bin/monitor-server \
  --db /var/lib/monitor/monitor.db admin set-password
```

Put the Server behind Caddy or Nginx for public HTTPS. TLS/ACME is not part of
Monitor. For Nginx, proxy to `http://127.0.0.1:25774`, preserve `Host`, and set
`X-Real-IP $remote_addr`; Caddy's `reverse_proxy 127.0.0.1:25774` is sufficient
and may explicitly set `X-Real-IP {remote_host}`. Never trust a client-supplied
`X-Real-IP` on a public listener.

## Agent installation (explicit release)

Create a root-owned environment file with mode `0600` containing the Server
URL and the one-time agent token:

```ini
MONITOR_SERVER=https://monitor.example.com
MONITOR_TOKEN=64-lowercase-hex-characters
```

Install the exact release and preserve the token outside argv, URLs, logs, and
shell history:

```sh
sudo ./scripts/install-monitor.sh --version v0.1.1 --component agent \
  --agent-env /root/monitor-agent.env
```

The Agent unit runs as `monitor-agent:monitor-agent`, has only `CAP_NET_RAW`
for raw ICMP, and has no inbound listener or command/file-write capability.
If the environment file is not configured, installation leaves the Agent
stopped and prints only the file path.

## Update and uninstall

Updates require an explicit version and verify the same-release checksum before
staging a binary. The old binary is restored if restart or health verification
fails; the database, Agent environment, and a custom Server listener are all
preserved:

```sh
sudo ./scripts/update-monitor.sh --version v0.1.1 --component all
```

Default uninstall stops and removes Monitor units and binaries, removes the
Agent secret file, and preserves Server data. To delete only the explicitly
named Server data directory, use the destructive flag:

```sh
sudo ./scripts/uninstall-monitor.sh --component all
sudo ./scripts/uninstall-monitor.sh --component server --purge
```

Uninstall never changes reverse-proxy configuration or unrelated services.

## Troubleshooting and data safety

If installation reports that the chosen port is occupied by an unknown
service, stop and identify that service yourself; the installer never kills or
replaces it. Inspect service state with `systemctl status monitor-server` or
`journalctl -u monitor-server` and `journalctl -u monitor-agent`.

Because the Server uses SQLite WAL mode, stop `monitor-server` before making a
simple file backup, or use SQLite's `.backup` operation. Do not copy only the
main database file while it is running. There is no automatic update daemon.
