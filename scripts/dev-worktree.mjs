#!/usr/bin/env node
// Write this checkout's `.claude/launch.json`, with ports nothing else is using.
//
// Two checkouts cannot share the app's ports, and the four environment
// variables that separate them have to agree with each other: the client's
// `VITE_API_BASE` must name the server's `OSP_BIND`, and the server's
// `OSP_EXTRA_ORIGINS` must name the client's `OSP_DEV_PORT` or the browser
// refuses every response (which presents as "cannot reach the server", not as a
// port problem). Agreeing on four values by hand, per worktree, is the kind of
// arithmetic that is done wrong once and debugged for an hour.
//
//   node scripts/dev-worktree.mjs            # write it, refusing to clobber
//   node scripts/dev-worktree.mjs --dry-run  # print it and write nothing
//   node scripts/dev-worktree.mjs --force    # overwrite what is there
//
// `launch.json` is machine-specific and git-ignored, so this script is careful
// to *derive* every machine-specific value rather than carry one:
//
//   - the node binary is `process.execPath` — the very node running this, which
//     is the one that will run vite, so it cannot be the wrong one;
//   - `PATH` is that binary's directory prepended to the inherited `PATH`,
//     because the preview tool spawns without a shell profile and vite needs to
//     find node;
//   - the ports are probed, not assigned, so two worktrees started minutes apart
//     need no coordination and no convention to remember;
//   - the data directory is *not* derived. Mapping %APPDATA% / ~/Library /
//     $XDG_DATA_HOME here would duplicate what the `dirs` crate already does on
//     the Rust side, and a copy of that mapping is a copy that can drift. It
//     goes under the worktree instead (`.dev-data/`, git-ignored): a scratch
//     checkout should not be writing tournaments into the per-user directory
//     the real app uses, and putting the backups there too is what stops a
//     second server from listing the first one's deleted tournaments.
//
// The full environment-variable reference is the module comment at the top of
// crates/server/src/main.rs. This script sets the six that differ per checkout.

import { createServer } from "node:net";
import { mkdirSync, existsSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { delimiter } from "node:path";

const repo = dirname(dirname(fileURLToPath(import.meta.url)));
const args = new Set(process.argv.slice(2));
const dryRun = args.has("--dry-run");
const force = args.has("--force");

/** Whether `port` can be bound on `host` right now. */
function free(port, host) {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.once("error", () => resolve(false));
    probe.once("listening", () => probe.close(() => resolve(true)));
    probe.listen(port, host);
  });
}

/**
 * The first port from `start` that is free on both loopback spellings.
 *
 * Both, because the two servers do not agree: axum binds `127.0.0.1` and vite
 * binds `localhost`, which resolves to `::1` first on a dual-stack machine. A
 * port free on one and taken on the other is exactly the confusing half-failure
 * this is meant to avoid.
 */
async function firstFreePort(start, label) {
  for (let port = start; port < start + 100; port++) {
    if ((await free(port, "127.0.0.1")) && (await free(port, "::1"))) return port;
  }
  throw new Error(`no free ${label} port in ${start}..${start + 99}`);
}

/**
 * The inherited `PATH` with the running node's directory first, and no entry
 * twice.
 *
 * The preview tool spawns without a shell profile, so the entry has to be there
 * explicitly. Deduplicated because an inherited `PATH` on a developer machine
 * usually already contains node several times over, and a launch file with a
 * 4kB `PATH` in it is one nobody will ever read. Case-insensitively on Windows,
 * where `C:\Foo` and `c:\foo` are the same directory.
 */
function pathWithNodeFirst() {
  const fold = (entry) => (process.platform === "win32" ? entry.toLowerCase() : entry);
  const seen = new Set();
  return [dirname(process.execPath), ...(process.env.PATH ?? "").split(delimiter)]
    .map((entry) => entry.replace(/[\\/]+$/, ""))
    .filter((entry) => entry && !seen.has(fold(entry)) && seen.add(fold(entry)) !== undefined)
    .join(delimiter);
}

const api = await firstFreePort(3000, "API");
const vite = await firstFreePort(5173, "dev-server");
const nodeBin = process.execPath;
const dataDir = join(repo, ".dev-data", "tournaments");
const backupDir = join(repo, ".dev-data", "backups");

const config = {
  version: "0.0.1",
  configurations: [
    {
      name: "frontend",
      runtimeExecutable: nodeBin,
      runtimeArgs: ["node_modules/vite/bin/vite.js"],
      cwd: "frontend",
      port: vite,
      env: {
        PATH: pathWithNodeFirst(),
        OSP_DEV_PORT: String(vite),
        VITE_API_BASE: `http://127.0.0.1:${api}`,
      },
    },
    {
      name: "server",
      runtimeExecutable: "cargo",
      runtimeArgs: ["run", "-p", "osp-server"],
      port: api,
      env: {
        OSP_BIND: `127.0.0.1:${api}`,
        OSP_DATA_DIR: dataDir,
        OSP_BACKUP_DIR: backupDir,
        // Both spellings: which one the browser sends depends on the URL the
        // page was opened at, and both are offered by vite's own banner.
        OSP_EXTRA_ORIGINS: `http://localhost:${vite},http://127.0.0.1:${vite}`,
      },
    },
  ],
};

const target = join(repo, ".claude", "launch.json");
const json = JSON.stringify(config, null, 2) + "\n";

if (dryRun) {
  process.stdout.write(json);
  process.exit(0);
}

if (existsSync(target) && !force) {
  console.error(
    `${target} already exists.\n` +
      "  Refusing to overwrite it: a checkout that has one has usually had it\n" +
      "  tuned by hand, and this script cannot tell which parts. Pass --force to\n" +
      "  replace it, or --dry-run to see what it would have written.",
  );
  process.exit(1);
}

mkdirSync(dirname(target), { recursive: true });
mkdirSync(dataDir, { recursive: true });
mkdirSync(backupDir, { recursive: true });
writeFileSync(target, json);

console.log(`Wrote ${target}`);
console.log(`  API        127.0.0.1:${api}`);
console.log(`  dev server localhost:${vite}`);
console.log(`  data       ${dataDir}`);
console.log("");
console.log("Run `npm ci` in frontend/ if this checkout has no node_modules yet,");
console.log("then start both with the preview tool's `server` and `frontend` names.");
