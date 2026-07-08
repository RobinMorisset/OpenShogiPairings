# Deploying the OpenShogiPairings server

This is the hosted **remote mode** from
[`docs/multi-referee-internet.md`](../docs/multi-referee-internet.md): one
always-on server with a public HTTPS URL that referees open in a browser and log
into with a shared password. It serves both the API and the app (the SPA),
same-origin, and persists the tournament to disk so a restart loses nothing.

You need a host with a public IP and a domain name pointed at it (an A/AAAA DNS
record). Everything below assumes that domain is `tournament.example.com` —
substitute your own.

## Configuration (environment variables)

The server is configured entirely through the environment:

| Variable         | Purpose                                                        | Default            |
| ---------------- | ------------------------------------------------------------- | ------------------ |
| `OSP_PASSWORD`   | Shared referee password gating the whole API. **Set this.**   | *(unset = open)*   |
| `OSP_BIND`       | Address to listen on.                                          | `127.0.0.1:3000`   |
| `OSP_STATIC_DIR` | Directory of the built SPA to serve same-origin.              | *(unset = API only)* |
| `OSP_DATA_FILE`  | File the tournament is loaded from and written through to.     | *(unset = in-memory)* |

Leaving `OSP_PASSWORD` unset runs the API open — only acceptable on a trusted
machine, never on a public host.

## Building the SPA for same-origin

The hosted server serves the SPA itself, so the client must talk to the API at
the same origin. Build the frontend with an **empty** API base:

```sh
cd frontend
VITE_API_BASE="" npm ci && VITE_API_BASE="" npm run build   # outputs frontend/dist
```

Point `OSP_STATIC_DIR` at that `frontend/dist`. (The Docker image below does this
for you.)

## Option A — Docker

Builds the binary and SPA and ships both. From the repo root:

```sh
docker build -f deploy/Dockerfile -t osp-server .
docker run -d --name osp \
  -p 127.0.0.1:3000:3000 \
  -e OSP_PASSWORD='your-long-shared-referee-password' \
  -v osp-data:/var/lib/osp \
  osp-server
```

Then run Caddy (below) in front for TLS. Publishing to `127.0.0.1:3000` keeps the
container reachable only from the host, i.e. only via Caddy.

## Option B — bare binary + systemd

1. Build: `cargo build --release -p osp-server` and the SPA (above).
2. Copy `target/release/osp-server` → `/opt/osp/osp-server` and
   `frontend/dist` → `/opt/osp/dist`.
3. Create the `osp` user and `/var/lib/osp` (owned by `osp`).
4. Put the password in `/etc/osp/osp.env` (`chmod 600`):
   `OSP_PASSWORD=your-long-shared-referee-password`
5. Install the unit:
   ```sh
   sudo cp deploy/osp-server.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now osp-server
   ```

## TLS with Caddy (both options)

[`Caddyfile`](./Caddyfile) reverse-proxies the domain to the loopback server and
obtains a Let's Encrypt certificate automatically:

```sh
sudo caddy run --config deploy/Caddyfile
```

(or install Caddy as a service). Once DNS resolves and Caddy has its certificate,
referees open `https://tournament.example.com/`, enter the shared password, and
collaborate. The tournament is saved to `OSP_DATA_FILE` on every change; back up
that file (and the server-side backups under the data directory) as part of
whatever backs up the host.

## Running a second tournament

One server instance holds one live tournament (see the design doc's scope
decision). For a second concurrent tournament, run a second instance — its own
container/service on another port and subdomain, with its own password and data
file.
