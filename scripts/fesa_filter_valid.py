#!/usr/bin/env python3
"""Move FESA result files that `osp-sim` can't use into `fesa_results/invalid/`.

A small fraction of the FESA corpus (~1-2%) is mis-generated in ways the importer
or the round reconstruction rejects. Rather than have every downstream step trip
over them, this runs the real acceptance test — `osp-sim --results FILE --runs 1`
— on each file and relocates the ones that fail (non-zero exit or timeout) into a
single `invalid/` directory, prefixed with their season so names stay unique.

Runs the checks in a thread pool; each is an independent short subprocess, so the
GIL is not the bottleneck.

Usage:
    scripts/fesa_filter_valid.py [--root test_files/fesa_results]
                                 [--sim target/release/osp-sim(.exe)]
                                 [--jobs N] [--timeout 60]
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import os
import shutil
import subprocess
import sys
from pathlib import Path


def find_sim(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    for cand in ("target/release/osp-sim.exe", "target/release/osp-sim"):
        if Path(cand).exists():
            return Path(cand)
    sys.exit("osp-sim binary not found; build it or pass --sim")


def check(sim: Path, path: Path, timeout: float) -> tuple[Path, bool, str]:
    """(path, ok, reason). ok=True if osp-sim accepts the file."""
    try:
        p = subprocess.run(
            [str(sim), "--results", str(path), "--runs", "1"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return path, False, "timeout"
    if p.returncode == 0:
        return path, True, ""
    reason = p.stderr.decode("utf-8", "replace").strip().splitlines()
    return path, False, (reason[-1] if reason else f"exit {p.returncode}")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", default="test_files/fesa_results")
    ap.add_argument("--sim", default=None)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--timeout", type=float, default=60.0)
    args = ap.parse_args()

    sim = find_sim(args.sim)
    root = Path(args.root)
    invalid_dir = root / "invalid"

    # Every .txt under a season directory (skip invalid/ itself so re-runs are
    # idempotent).
    files = sorted(
        p
        for p in root.glob("*/*.txt")
        if p.parent.name != "invalid"
    )
    if not files:
        sys.exit(f"no result files under {root}/ (run fesa_fetch_all.py first)")
    print(f"checking {len(files)} files with {args.jobs} jobs …", file=sys.stderr)

    bad: list[tuple[Path, str]] = []
    done = 0
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        futs = [ex.submit(check, sim, f, args.timeout) for f in files]
        for fut in cf.as_completed(futs):
            path, ok, reason = fut.result()
            done += 1
            if not ok:
                bad.append((path, reason))
            if done % 250 == 0:
                print(f"  {done}/{len(files)} ({len(bad)} rejected)", file=sys.stderr)

    invalid_dir.mkdir(parents=True, exist_ok=True)
    for path, reason in sorted(bad):
        dest = invalid_dir / f"{path.parent.name}__{path.name}"
        shutil.move(str(path), str(dest))
        print(f"REJECT {path.parent.name}/{path.name}: {reason}")

    total = len(files)
    print(
        f"\n{len(bad)}/{total} rejected ({100 * len(bad) / total:.2f}%) "
        f"moved to {invalid_dir}/; {total - len(bad)} valid",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
