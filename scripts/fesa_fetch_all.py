#!/usr/bin/env python3
"""Fetch every FESA season's results page and split it into per-tournament files.

Drives `fesa_html_to_results` over the whole published corpus: Fall + Spring for
each year in [2000, 2025], plus Spring 2026. Each season's tournaments land in
their own directory under `test_files/fesa_results/` — e.g. `spring2023/`,
`fall2023/` — so a later benchmarking step can sample realistic tournaments from
the pool (see `scripts/gen_fake_tournament.py`).

The whole `test_files/fesa_results/` tree is gitignored; this regenerates it on
demand rather than tracking the (large, ~1.5%-corrupt) batch.

Usage:
    scripts/fesa_fetch_all.py [--outroot test_files/fesa_results] [--sleep 1.0]
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import sys
import time
import urllib.error
from pathlib import Path

# Load the sibling converter module (its file name is a valid module name).
_HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location(
    "fesa_html_to_results", _HERE / "fesa_html_to_results.py"
)
fhr = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fhr)

FESA_URL = "https://fesashogi.eu/old/index.php?mid=4&dateid={season}+{year}&tournamentid=all"


def seasons() -> list[tuple[str, int]]:
    """(season, year) pairs to fetch: Fall+Spring 2000-2025, then Spring 2026."""
    out: list[tuple[str, int]] = []
    for year in range(2000, 2026):
        out.append(("Spring", year))
        out.append(("Fall", year))
    out.append(("Spring", 2026))
    return out


def convert_page(doc: str, outdir: str) -> tuple[int, int]:
    """Split one already-fetched season page into result files in `outdir`.

    Mirrors `fesa_html_to_results.main`'s table loop, but takes the page text so
    the driver controls fetching (and can space requests out politely)."""
    os.makedirs(outdir, exist_ok=True)
    written = skipped = 0
    for table in fhr.TABLE_RE.findall(doc):
        title = fhr.title_from_table(table)
        rows = list(fhr.player_rows(table))
        if title is None or not rows:
            skipped += 1
            continue
        written += 1
        path = os.path.join(outdir, fhr.slugify(title, written))
        with open(path, "wb") as f:
            f.write(fhr.format_tournament(title, rows).encode("latin-1"))
    return written, skipped


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--outroot", default="test_files/fesa_results")
    ap.add_argument(
        "--sleep",
        type=float,
        default=1.0,
        help="seconds to wait between season page fetches (be polite to FESA)",
    )
    args = ap.parse_args()

    total_files = 0
    for i, (season, year) in enumerate(seasons()):
        url = FESA_URL.format(season=season, year=year)
        outdir = os.path.join(args.outroot, f"{season.lower()}{year}")
        try:
            doc = fhr.load_page(url)
        except urllib.error.URLError as e:
            print(f"{season} {year}: FETCH FAILED ({e})", file=sys.stderr)
            continue
        written, skipped = convert_page(doc, outdir)
        total_files += written
        print(
            f"{season} {year}: {written} files -> {outdir}"
            + (f" ({skipped} non-tournament tables skipped)" if skipped else "")
        )
        if i + 1 < len(seasons()) and args.sleep > 0:
            time.sleep(args.sleep)

    print(f"\nTotal: {total_files} result files under {args.outroot}/", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
