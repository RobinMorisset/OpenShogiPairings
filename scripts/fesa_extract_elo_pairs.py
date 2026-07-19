#!/usr/bin/env python3
"""Pool per-player samples from the valid FESA corpus into one sampling table.

Runs `fesa-dump` (the authoritative importer) over every accepted result file and
collects, per player, everything the fake-tournament generator needs to draw a
realistic synthetic player:

- `start` — pre-tournament pairing ELO, or `null` if the player was unrated
  before the event (then `end` is their assigned `*` rating).
- `end`   — post-tournament true strength (`start + (+/-)`, or the `*` rating),
  the oracle's center.
- `rounds`— the source tournament's round count.
- `absent`— the 1-based rounds that player sat out.

`start`/`end` are kept **together per player** (not as two independent pools) so
their correlation — a player ends near where they started — survives sampling;
likewise `absent` rides along with the same player, preserving any tie between
strength and attendance and the within-player round correlation.

The same player appears as many times as they show up across the corpus; that is
intended — sampling with replacement then reproduces real frequency.

Output is a single JSON file (default `test_files/fesa_results/fesa_elo_pairs.json`
— alongside the gitignored corpus it is derived from, so it is not committed but
travels in the same archive). Each player is a compact 4-element array
`[start, end, rounds, absent]` (keys would triple the file), so:
`{"meta": {...}, "players": [[1500,1515,9,[]], [null,2337,9,[8,9]], …]}`.

Usage:
    scripts/fesa_extract_elo_pairs.py [--root test_files/fesa_results]
        [--dump target/release/fesa-dump(.exe)] [--out scripts/fesa_elo_pairs.json]
        [--jobs N]
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import subprocess
import sys
from pathlib import Path


def find_dump(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    for cand in ("target/release/fesa-dump.exe", "target/release/fesa-dump"):
        if Path(cand).exists():
            return Path(cand)
    sys.exit("fesa-dump binary not found; build it or pass --dump")


def dump_file(dump: Path, path: Path) -> list[dict]:
    """Player records from one file, or [] if fesa-dump fails (shouldn't after
    the acceptance filter, but stay robust)."""
    p = subprocess.run(
        [str(dump), str(path)], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    if p.returncode != 0:
        return []
    d = json.loads(p.stdout)
    n_rounds = d["n_rounds"]
    out = []
    for pl in d["players"]:
        if pl["strength"] is None:
            continue
        # Compact record: [start, end, rounds, absent]. start is None for a
        # pre-unrated player (then end is their assigned `*` rating).
        out.append([pl["rating"], round(pl["strength"]), n_rounds, pl["absent_rounds"]])
    return out


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", default="test_files/fesa_results")
    ap.add_argument("--dump", default=None)
    ap.add_argument("--out", default="test_files/fesa_results/fesa_elo_pairs.json")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    args = ap.parse_args()

    dump = find_dump(args.dump)
    root = Path(args.root)
    files = sorted(p for p in root.glob("*/*.txt") if p.parent.name != "invalid")
    if not files:
        sys.exit(f"no valid result files under {root}/")
    print(f"dumping {len(files)} files with {args.jobs} jobs …", file=sys.stderr)

    players: list[dict] = []
    done = 0
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        for recs in ex.map(lambda f: dump_file(dump, f), files):
            players.extend(recs)
            done += 1
            if done % 500 == 0:
                print(f"  {done}/{len(files)} ({len(players)} players)", file=sys.stderr)

    rated = sum(1 for p in players if p[0] is not None)
    with_absence = sum(1 for p in players if p[3])
    meta = {
        "record_format": "[start, end, rounds, absent]; start=null if pre-unrated",
        "source_files": len(files),
        "n_players": len(players),
        "rated_start": rated,
        "unrated_start": len(players) - rated,
        "players_with_absence": with_absence,
    }
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump({"meta": meta, "players": players}, f, separators=(",", ":"))

    size_mb = out_path.stat().st_size / 1e6
    print(
        f"\nwrote {len(players)} player records ({rated} rated, "
        f"{len(players) - rated} unrated-start; {with_absence} with absences) "
        f"to {out_path} ({size_mb:.2f} MB)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
