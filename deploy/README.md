# Deploying the OpenShogiPairings server

This is the hosted **remote mode** from
[`docs/multi-referee-internet.md`](../docs/multi-referee-internet.md): one
always-on server with a public HTTPS URL that referees open in a browser, pick a
tournament from, and log into. It serves both the API and the app (the SPA),
same-origin, and persists every tournament to disk so a restart loses nothing.

One instance holds **any number of tournaments** at once (see
[`docs/multi-tournament.md`](../docs/multi-tournament.md), which superseded the
one-tournament-per-instance scope of the doc above).

You need a host with a public IP and a domain name pointed at it (an A/AAAA DNS
record). Everything below assumes that domain is `tournament.example.com` —
substitute your own.

## Configuration (environment variables)

The server is configured entirely through the environment:

| Variable              | Purpose                                                                                  | Default              |
| --------------------- | ---------------------------------------------------------------------------------------- | -------------------- |
| `OSP_ADMIN_PASSWORD`  | Gates *creating* / importing tournaments and the FESA ratings proxy. **Set this.**        | *(unset = open)*     |
| `OSP_BIND`            | Address to listen on.                                                                      | `127.0.0.1:3000`     |
| `OSP_STATIC_DIR`      | Directory of the built SPA to serve same-origin.                                           | *(unset = API only)* |
| `OSP_DATA_DIR`        | Directory holding one `{id}.json` (+ `{id}.auth.json`) per tournament.                     | *(unset = in-memory)* |
| `OSP_BACKUP_DIR`      | Directory holding one folder of rotating automatic backups per tournament.                 | the per-user data dir |
| `OSP_BACKUP_RETENTION_DAYS` | How long a *deleted* tournament's backups are kept before they are swept. `0` deletes them with it. | `30` |

### Who can do what

There is no single password over the whole API. Access comes in two pieces:

- **`OSP_ADMIN_PASSWORD`** — instance-wide, and only for the instance-wide
  capabilities: creating or importing a tournament, and the FESA ratings proxy
  that powers registration autocomplete. Leaving it unset lets anyone who finds
  the URL create tournaments on your host; set it before exposing the server.
  Note it is not purely an operator secret: a referee who has not entered it can
  still run their tournament, but cannot create one, and loses FESA autocomplete
  during registration (it degrades silently). Share it with the people who set
  tournaments up.
- **Each tournament's own password**, chosen when the tournament is created,
  gating that tournament alone. A referee with tournament A's password cannot
  read or edit tournament B. A tournament created without one is editable by
  anyone who can reach the server.

The *list* of tournaments (names, and whether each is password-protected) is
public by design — the picker has to render before anyone can log in anywhere.
Don't put anything confidential in a tournament's name.

`OSP_BACKUP_DIR` is worth setting explicitly on a server: unset, backups go to
the service user's per-user data directory, which under a `systemd` unit or in a
container is usually not where you look for them — and not on the volume you
back up. That directory also holds deleted tournaments for
`OSP_BACKUP_RETENTION_DAYS`: deleting one keeps a final backup of the state it
was deleted in, and only a sweep past the retention (at startup and after each
deletion) removes it. Size the volume for a month of them, or lower the
retention.

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
  -e OSP_ADMIN_PASSWORD='your-long-admin-password' \
  -v osp-data:/var/lib/osp \
  osp-server
```

The image points `OSP_DATA_DIR` and `OSP_BACKUP_DIR` at `/var/lib/osp/tournaments`
and `/var/lib/osp/backups`, so the single `osp-data` volume above holds both.

Then run Caddy (below) in front for TLS. Publishing to `127.0.0.1:3000` keeps the
container reachable only from the host, i.e. only via Caddy.

## Option B — bare binary + systemd

1. Build: `cargo build --release -p osp-server` and the SPA (above).
2. Copy `target/release/osp-server` → `/opt/osp/osp-server` and
   `frontend/dist` → `/opt/osp/dist`.
3. Create the `osp` user and `/var/lib/osp` (owned by `osp`). The unit points
   the tournaments and backups at subdirectories of it; the server creates them
   on first use.
4. Put the admin password in `/etc/osp/osp.env` (`chmod 600`):
   `OSP_ADMIN_PASSWORD=your-long-admin-password`
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
referees open `https://tournament.example.com/`, pick their tournament, enter its
password, and collaborate. Every tournament is written to `OSP_DATA_DIR` on every
change; back up that directory — and `OSP_BACKUP_DIR` alongside it — as part of
whatever backs up the host.

## Running several tournaments

One instance holds as many as you like: create each from the picker, and give
each its own password. Only isolation calls for a second instance — separate
hosts, separate admin passwords, or keeping one federation's data off another's
disk — in which case run its own container/service on another port and
subdomain, with its own `OSP_DATA_DIR` and `OSP_BACKUP_DIR`.
