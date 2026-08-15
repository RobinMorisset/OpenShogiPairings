# OpenShogiPairings

[![CI](https://github.com/RobinMorisset/OpenShogiPairings/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/RobinMorisset/OpenShogiPairings/actions/workflows/ci.yml)
[![coverage](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/RobinMorisset/f05cf66791d190fa3defbb2ebf1dbcb6/raw/osp-coverage.json)](#coverage)

Tournament management software for shogi, built to fit shogi's needs rather than
reusing go/chess software. Built around a **client/server** architecture so
that multiple referees can edit the same tournament live, from their own
machines — as a portable single-file desktop app, or hosted on the internet and
reached from a browser.

## Download

Ready-to-run builds for **Windows** and **macOS** are on the
[**Releases**](https://github.com/RobinMorisset/OpenShogiPairings/releases/latest)
page:

- **Windows** — download the `.exe` (or the `.msi` installer) and double-click.
  No install or command line needed: the server, UI, and logic are all embedded
  in the single file.
- **macOS** — download the `.dmg`, open it, and drag OpenShogiPairings into
  Applications. It's a universal build (both Apple Silicon and Intel Macs).

> **You'll see an "unknown publisher" / "damaged" warning the first time.** These
> builds are **not code-signed** — I haven't paid for the Apple and Windows
> signing certificates — so the operating system shows a scary-looking warning on
> first launch. The app is safe; here's how to get past it:
> - **Windows**: on the blue SmartScreen popup, click **More info → Run anyway**.
> - **macOS**: right-click (or Ctrl-click) the app and choose **Open**, then
>   confirm in the dialog. If macOS insists the app is "damaged", run this once in
>   Terminal and then open it normally:
>   ```sh
>   xattr -dr com.apple.quarantine /Applications/OpenShogiPairings.app
>   ```

## Features

A tour of what OpenShogiPairings does for the referee actually running a
tournament. For how it's built, see [Architecture](#architecture) below.

### Shogi-specific

- **MacMahon groups**, with thresholds on either ELO rating or dan/kyu grade
  (freely mixed), and an optional **degressive ("accelerated") schedule** that
  lets a starting-point head start fade out over the first few rounds.
  Optionally, MacMahon can be awarded from a **live ELO estimate** rather than
  the registration rating, so a player's starting points track their estimated
  strength round by round (see the pairing options below).
- **Hybrid direct-elimination cup** alongside the Swiss (the French / European
  Championship format): top 8/16/32/64 eligible players play a seeded bracket
  over the first rounds, eliminated players drop back into the Swiss, and the
  Standings tab shows the podium medals. Optionally the bracket is fed by a
  **qualification round** (the German Championship format): half the bracket is
  pre-qualified and plays the open in round 1 while the next players play off
  for the remaining slots.
- **Handicap games**, with a recommended handicap (FFS Annexe 7, from the
  rating gap) suggested automatically, and draws-before-the-decisive-game
  recorded for ELO purposes.
- **FESA rating list integration**: registration autocompletes name, ELO, and
  grade from the federation's list, refreshable on demand; the tournament
  cross-table can be exported as an **American Grid** for the federation's
  ELO update, headed by the tournament's name, place, dates and time control
  (all but the name entered in the settings).
- **Licence check**: point the Players tab at a CSV list of the players whose
  federation licence is paid up (Last name, First name — whatever a federation's
  back office exports), pick a nationality, and it names the registered players
  of that nationality who are missing from it. Since these exports are typed by
  hand, each of them also carries any list entry **one character from their
  name** — a swap, a dropped or extra letter — so a `DUPOND` for `DUPONT` reads
  as the misspelling it probably is rather than as an unpaid fee. Read-only: it
  tells the referee who to chase, it doesn't touch the roster, and a near miss
  never counts as a licence found.
- **Manual point adjustments** (a bonus or penalty with a mandatory reason),
  for corrections outside the normal scoring.
- **Two-round "long games"** (niche): for tournaments that give the top boards
  double the time control, a board can be flagged to span two rounds — its two
  players sit out the intervening round's pairing and the winner scores two
  points. Off by default.

### Exotic pairing options

- **Experimental pure ELO pairing mode**: ignores MacMahon and Swiss score
  groups entirely and pairs each round purely to minimize the estimated ELO gap,
  a "continuous Swiss" for fields where a smooth strength axis fits better than
  integer points.
- **Estimate-based MacMahon**: a lighter hybrid that leaves pairing alone and
  instead awards MacMahon starting points from a live ELO estimate — so the
  groups themselves react to results, while plain Swiss pairing runs as usual.
  Enabled from the MacMahon settings; needs at least one ELO threshold.
- **Club protection**, avoiding pairing players from the same club, optionally
  limited to the first N rounds and with specific clubs (e.g. the host club)
  exempted.
- **Nationality protection**, the same knob one rule tier weaker: avoid pairing
  players of the same nationality, again optionally limited to the first N
  rounds and with nationalities (e.g. the host country) exempted. When only one
  of the two can be honoured, the club clash is the one avoided.
- **Airtight groups**: for the first N rounds, forbid pairing players with a
  different MacMahon point total, ahead of the usual score-gap penalty.
- **Floater selection style** — classic Swiss (the strongest of the lower
  group floats up) or median Swiss (the median floats up) — when a score
  group must pair across group lines.

### Convenience & ergonomics

- **"Why these pairings?"**: every round explains itself — a per-board
  compromise ledger, a round-level report of which rules had to bend, and a
  counterfactual probe ("why weren't these two paired?" / "why were they?")
  that shows exactly what forcing or forbidding a pairing would cost. The
  ledger is recorded when the round is paired, so a later correction can't
  rewrite it; when the data it cites has since been edited, it says so.
- **Click-to-edit** any player cell in place; forced pairings and a forced
  bye when hand-tuning a round draft.
- **Print** pairings, **save/load** a tournament as a portable JSON file.
- **UI in nine languages** (English, French, German, Japanese, Russian,
  Belarusian, Ukrainian, Slovak, Polish), light/dark theme.
- Runs as a **single portable desktop executable** (no install, no command
  line) or in a plain browser against a hosted server — same app either way.

### Safety

- **Multi-referee live collaboration over the internet**: several referees can
  edit the same tournament from different machines at once, with changes
  appearing live and simultaneous edits caught by conflict detection rather
  than silently overwritten.
- **Password-protected tournaments** (each with its own password, set at
  creation) plus a separate admin password gating who can create tournaments
  on a shared server — so one hosted instance can safely serve many
  tournaments/federations at once, picked from a list.
- **Full undo history** and **automatic backups** at every round-lifecycle
  transition (finalize / prepare / start / complete / cancel), restorable
  from the UI.
- **Guarded round lifecycle**: the next round can't start until the current
  one's games are all recorded, registration can't be skipped, and destructive
  actions (discard, delete) always ask for confirmation first.

## Limitations

- **Not yet battle-tested:** this project has not been used in a real tournament yet.
- **Not exact FIDE-style Swiss:** like OpenGotha or pairgoth, this software uses an engine
  that searches for a global optimum for pairings. This is intrinsically incompatible with
  the intricate rules governing the order in which pairings are to be tried under FIDE's
  Swiss regulations. All that can be guaranteed is that the resulting pairings are no worse,
  by a number of metrics, than those a more classical Swiss pairing program would find.
- **Not protected from untrusted referees:** the software assumes that all of the referees
  with access to a tournament can be trusted. In particular, it has no detailed log of
  which referee took which action.
- **Portability:** this project has only been tested on one Windows 10 desktop and one macOS
  laptop. It should run fine on Linux, and the web UI should work in any modern browser, but
  neither has been tested yet.

## Architecture

![The crate stack: integer-blossom at the bottom, osp-core on it, osp-server and
osp-sim side by side above, the frontend on osp-server. Everything but osp-sim is
packaged into the Tauri executable.](docs/reference/architecture.svg)

Rust does the correctness-critical work, TypeScript the interface. The domain
and the pairing engine live in `osp-core`, on top of a standalone blossom
(min-weight perfect matching) solver, `integer-blossom`; `osp-server` wraps them
in an axum HTTP API that is the source of truth for any number of tournaments at
once. That server is both a standalone binary — for browser development and the
hosted remote deployment — and a library, which is what the Tauri desktop app
links and starts in-process, so the packaged app ships as a single
self-contained executable. The Svelte frontend is the same code either way.

- [`docs/reference/architecture.md`](docs/reference/architecture.md) — the
  crates and what each is for, the multi-tournament registry and its two-level
  auth, public read-only access, where save files and backups live, live
  multi-referee sync, the pairing engine, and the UI.
- [`docs/reference/api.md`](docs/reference/api.md) — every HTTP route.
- [`docs/`](docs/README.md) — everything else: guides, proposals, and archived
  design docs.

## Prerequisites

- **Rust** (stable, ≥ 1.90) via [rustup](https://rustup.rs) — the highest
  `rust-version` in the dependency graph (`typed-index-collections`); the
  workspace's own floor is `crates/matching`'s 1.87
- **Node.js** ≥ 22.22.2 (or ≥ 24.15.0, or ≥ 26) with npm — the range in
  `frontend/package.json`'s `engines`, enforced at install time by
  `frontend/.npmrc`
- For the Tauri desktop client: platform WebView (WebView2 is preinstalled on
  current Windows; `webkit2gtk` on Linux) — see the
  [Tauri prerequisites](https://tauri.app/start/prerequisites/).

## Running (development)

Two processes; run them in separate terminals.

**1. Server** (listens on `http://127.0.0.1:3000`):

```sh
cargo run -p osp-server
```

**2. Web UI** (Vite dev server on `http://localhost:5173`):

```sh
cd frontend
npm install   # first time only
npm run dev
```

Open <http://localhost:5173>. The page reports whether it can reach the server.

### Desktop app

```sh
cd frontend
npm run tauri dev
```

The desktop app **embeds the server**, so you do *not* run `cargo run -p osp-server`
alongside it — it starts its own API in-process.

> `tauri dev` starts its own Vite dev server on port 5173 (via
> `beforeDevCommand`), so don't also have a browser `npm run dev` (or another
> preview) running on 5173 at the same time — the port is fixed (`strictPort`)
> and the second one will fail with "Port 5173 is already in use".

> **Rust changes need a restart.** `tauri dev` hot-reloads the frontend, but its
> file watcher covers `frontend/src-tauri` only — `src-tauri` is its own
> workspace (see the comment in its `Cargo.toml`) and the `crates/` it depends
> on are outside it. Edit `crates/core` or `crates/server` and the running app
> keeps the behaviour it was built with, silently, while the UI updates around
> it. Stop `tauri dev` and start it again to pick the change up.

## Packaging (Windows)

> To publish prebuilt Windows **and** macOS apps to the GitHub Releases page,
> don't build by hand — push a version tag and let CI do it. See
> [Cutting a release](docs/guides/cutting-a-release.md). The rest of this section is for
> producing a one-off local build.

Build the self-contained portable executable:

```sh
cd frontend
npm run tauri build -- --no-bundle
```

The result is a single file at
`frontend/src-tauri/target/release/OpenShogiPairings.exe` — server, UI, and logic
all embedded. Referees just double-click it; no install, no command line, no
separate server.

The one system dependency is **WebView2** (the browser engine the UI renders
in), which is preinstalled on all Windows 10 (2021+) and 11. To instead produce
an installer that bundles WebView2 for fully-offline install on any machine, drop
`--no-bundle` and set `bundle.windows.webviewInstallMode` to `offlineInstaller`
in [`tauri.conf.json`](frontend/src-tauri/tauri.conf.json).

## Testing

```sh
cargo test          # Rust workspace (core, server, sim, matching)
cd frontend && npm run check   # Svelte / TypeScript type-check
cd frontend && npm run lint    # ESLint (typescript-eslint + eslint-plugin-svelte)
cd frontend && npm test        # frontend unit tests (vitest)
```

Dev helpers (Windows/PowerShell) in [`scripts/`](scripts):

- `scripts/check.ps1` — runs `cargo test`, the frontend type-check
  (`npm run check`) and ESLint (`npm run lint`).
- `scripts/restart-server.ps1` — restart `osp-server` and wait until it responds
  (the running server doesn't hot-reload, so restart it after backend changes).

### Git hooks

Versioned hooks live in [`scripts/git-hooks/`](scripts/git-hooks) (not
`.git/hooks/`, which isn't checked in). Point git at them once per clone:

```sh
git config core.hooksPath scripts/git-hooks
```

- `pre-commit` — rustfmt (auto-formats staged `.rs` files and re-stages them, so
  a formatting nit doesn't fail the commit; a partially-staged file it would
  rewrite is reported and gates instead), `cargo clippy --workspace
  --all-targets -- -D warnings`, `svelte-check`, `eslint`, and an i18n
  locale-key check (`scripts/check-i18n-keys.mjs`, catching keys that exist in
  some locales but not all). Fast; keeps the tree clean commit by commit.
- `pre-push` — the full test suite (`cargo test --workspace` and the frontend
  tests, `npm test`), a check that the ts-rs bindings under
  `frontend/src/lib/generated/` match the Rust types they come from (the test
  run above is what writes them, so this is just a diff — and nothing else
  notices, since CI type-checks against the committed copy), a rustdoc pass with
  warnings denied (`cargo doc --workspace
  --no-deps --document-private-items`, catching doc links that stopped
  resolving — rustdoc never fails a build over one), plus the
  dependency-advisory check
  (`scripts/check-advisories.py`, the same one CI runs). Slower, so it only runs
  before sharing work. The advisory check needs
  [`cargo-audit`](https://github.com/rustsec/rustsec) (`cargo install
  cargo-audit`); without it the hook says so and skips rather than blocking the
  push, and CI catches what you miss.

### Dependency advisories

`scripts/check-advisories.py` audits **both** lockfiles — the workspace's and
`frontend/src-tauri`'s, which is outside the workspace and carries the whole
Tauri tree — and triages the results against the resolved dependency graph:

```sh
python3 scripts/check-advisories.py
```

`cargo audit` on its own reads a lockfile, and a lockfile is feature-blind: it
records a package's *optional* dependencies whether or not anything enables them,
so it reports crates that are never compiled into anything shipped. Rather than
carrying an ignore list — a claim about the dependency graph on the day it was
written, which keeps holding silently after it stops being true — the script asks
cargo which packages are actually in the graph and keeps only the advisories
whose package (name *and* version) is really there. Everything it dismisses is
printed on every run, so the reasoning stays visible; anything that fails to run
is an error, never a pass.

### Coverage

Rust coverage uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(`cargo install cargo-llvm-cov`, plus `rustup component add llvm-tools-preview`):

```sh
cargo llvm-cov -p osp-core --summary-only      # core only
cargo llvm-cov -p osp-server --summary-only    # server only
cargo llvm-cov --workspace --summary-only      # everything, incl. binaries
cargo llvm-cov --workspace --html              # HTML report in target/llvm-cov/html/index.html

# The exact figure the CI badge reports: the shipped library crates, with
# binary entry points (server/src/main.rs) excluded.
cargo llvm-cov -p osp-core -p integer-blossom -p osp-server \
  --ignore-filename-regex 'main\.rs$' --summary-only
```

**What the badge measures.** The `coverage` job in CI reports the shipped library
crates (`osp-core`, `integer-blossom`, `osp-server`) and *excludes* binary entry
points — `server/src/main.rs` and the whole `osp-sim` dev tool are 0%-by-nature
glue that would understate the real figure. The measured logic sits at ~95% line
coverage; the residual gaps are deliberately-untested server I/O (the live FESA
fetch in `ratings.rs`, the SSE stream in `live.rs`), not core logic. The job is
informational — it never fails the build.

**Enabling the badge (one-time GitHub setup).** The badge reads its number from a
GitHub Gist that CI keeps up to date. Two account-side steps, which the repo owner
must do (they can't be scripted here):

1. Create a **secret** Gist (any single file, e.g. `osp-coverage.json` with `{}`),
   and note its ID (the hash in its URL).
2. Create a [fine-grained PAT](https://github.com/settings/tokens) with **Gist**
   read/write, then in this repo's **Settings → Secrets and variables → Actions**
   add a secret `GIST_TOKEN` (the PAT) and a variable `COVERAGE_GIST_ID` (the ID).

With both in place, every push to `main` refreshes the number in the Gist
([`f05cf66…`](https://gist.github.com/RobinMorisset/f05cf66791d190fa3defbb2ebf1dbcb6)),
which the coverage badge at the top of this README reads via
[shields.io](https://shields.io).

The frontend uses [vitest](https://vitest.dev) (`npm test`) for unit tests but has
no coverage tooling wired up, by design — very little testable logic lives there.
