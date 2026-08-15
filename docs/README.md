# Documentation

Four directories, sorted by **how much you can trust what is in them**.

## [`reference/`](reference) — how the software works today

Maintained alongside the code: if one of these disagrees with the codebase, the
document is a bug.

- [`architecture.md`](reference/architecture.md) — the crates, the
  multi-tournament registry and its auth, public read-only access, where files
  live, live multi-referee sync, the pairing engine, the UI.
- [`api.md`](reference/api.md) — every HTTP route. Kept in sync in the same
  change as any route added, changed or removed.
- [`swiss-fold.md`](reference/swiss-fold.md) — the fold rule and why its penalty
  is quadratic.

## [`guides/`](guides) — how to do a thing

Procedures, for whoever is doing them.

- [`cutting-a-release.md`](guides/cutting-a-release.md)
- [`../deploy/README.md`](../deploy/README.md) — running the server yourself
  (Docker, systemd, Caddy), and the full environment-variable table. It lives
  next to the recipes it describes rather than in here.
- [`simulation-cli.md`](guides/simulation-cli.md) — `osp-sim`, the pairing-settings simulator.
- [`solver-scaling-benchmark.md`](guides/solver-scaling-benchmark.md) — how pairing cost scales with field size.
- [`desktop-startup-profiling.md`](guides/desktop-startup-profiling.md) — where desktop startup time goes.

## [`proposals/`](proposals) — not built yet

Designs for work that has not landed. Describes an intention, never the current
behaviour.

- [`blossom-v2-plan.md`](proposals/blossom-v2-plan.md) — sparse core with preserved trees.

## [`archive/`](archive) — historical, drifted

Design docs written before their feature was implemented and not maintained
since. **Do not trust the details.** They are kept for the rationale they
record — why a thing was built the way it was, which alternatives were rejected
and why — and are being folded into `reference/` a piece at a time, then
deleted. Until a document is gone, `reference/` and the code win over anything
in here.
