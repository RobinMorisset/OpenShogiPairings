---
name: run-locally
description: Start the API server and the Vite dev server to try the app or verify a change in a browser, and run a second checkout alongside the first without the two fighting over ports or data. Use when asked to run, start, serve or browser-verify the app; when a change needs looking at rather than only testing; or when working in a git worktree, which has no launch configuration of its own until one is generated. Covers the four failure modes that do not look like what they are.
---

# Running the app locally

Two processes: `osp-server` (the API) and Vite (the SPA). The preview tool starts
both, by the names in `.claude/launch.json` — `server` and `frontend`. Never
start them with a shell command; the preview tool owns their lifecycle and is
what the browser tools attach to.

The desktop app is a third way to run it and embeds its own server — do not also
start `osp-server` for it. See the README.

## In a fresh checkout or worktree

`.claude/launch.json` is git-ignored and machine-specific (absolute paths to
node, a `PATH` the preview tool's spawn environment lacks), so a worktree has
**no launch configuration at all**. Generate one:

```sh
node scripts/dev-worktree.mjs
```

It probes for free ports, derives everything machine-specific rather than
carrying it, puts that checkout's tournaments and backups under its own
`.dev-data/`, and wires the four variables that have to agree with each other. It
refuses to overwrite an existing `launch.json` — pass `--dry-run` to see what it
would write, `--force` if you really mean it.

Then `npm ci` in `frontend/` if that checkout has no `node_modules`, and start
the two by name.

## Four failures that do not look like what they are

**"Cannot reach the server" right after starting.** Usually neither a port nor a
CORS problem: `cargo run` takes ~20s to compile before it listens, and the page
probed while it was still building. Reload. If it persists, check the server's
own log — the preview tool captures it.

**"Cannot reach the server" that survives a reload, in a second checkout.** This
one *is* CORS. The API answers a named list of origins (`CROSS_ORIGIN_CLIENTS` in
`crates/server/src/lib.rs`), so a frontend on any port but 5173 is refused every
response and the browser reports it as unreachable. `OSP_EXTRA_ORIGINS` must name
that frontend's origin; the generator sets it.

**Tournaments vanishing on restart.** The server keeps everything in memory
unless `OSP_DATA_DIR` is set. Stopping it — to rebuild, say, since the running
binary locks `target/debug/osp-server.exe` — then loses every tournament created
since boot. To rebuild without stopping it, build into another target directory
(`CARGO_TARGET_DIR=…`). Backups survive regardless and can be re-imported through
the picker's *Load from file…*.

**A second server listing the first one's deleted tournaments.** `OSP_DATA_DIR`
separates the live tournaments; `OSP_BACKUP_DIR` separates the backups, and they
are separate settings on purpose. Set both, or the second checkout's picker shows
the first one's bin.

## Verifying a change in the browser

Prefer the text tools — `read_page`, `get_page_text`, `read_console_messages`,
`javascript_tool` for computed styles and geometry — over screenshots. Layout
bugs in this app are usually a number: a width, an offset, a specificity. Measure
the number, before and after, rather than describing the picture.

`javascript_tool` is for reading and for driving the page in ways the tools
cannot; it is not a way to make changes. Fix CSS in the component and let HMR
apply it.

Two traps when driving the referee app from a script: Svelte applies state
asynchronously, so read back after a tick rather than immediately; and its
component CSS is scoped, so a probe element you inject needs the same `s-…` class
as its neighbours or none of the component's rules apply to it.

## The full set of knobs

The canonical list of environment variables — with what each one does and what
happens when it is unset — is the module comment at the top of
**`crates/server/src/main.rs`**. Read it there rather than trusting a summary;
this file only says which of them differ per checkout:

| variable | why a second checkout needs its own |
|---|---|
| `OSP_BIND` | two servers cannot share a port |
| `OSP_DATA_DIR` | separates the tournaments |
| `OSP_BACKUP_DIR` | separates the backups, which `OSP_DATA_DIR` does not |
| `OSP_EXTRA_ORIGINS` | lets this server answer this checkout's frontend |
| `OSP_DEV_PORT` | Vite's port (`strictPort`, so it fails loudly rather than hopping) |
| `VITE_API_BASE` | points this frontend at this server |
