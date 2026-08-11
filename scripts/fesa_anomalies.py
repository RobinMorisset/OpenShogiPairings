#!/usr/bin/env python3
"""Write `fesa_results/ANOMALIES.md`: every corpus file the parser found fault with.

Two kinds of finding, both worth reporting upstream to the FESA site:

- **Rejected** — the importer refuses the file outright (these are the ones
  `fesa_filter_valid.py` moves into `invalid/`). Grouped by the parser's own
  error message, with the variable parts (line numbers, cell text, round counts)
  folded away so one group is one *kind* of defect.
- **Tolerated** — the file imports, but only because the parser works around
  something malformed in it, and information is dropped in the process. These
  stay in the corpus; the report is the only record that they are odd.

Rejections come from running the real importer (`fesa-dump`, one short
subprocess per file, in a thread pool). Tolerated findings are matched on the
text, mirroring what `normalize_cell_annotations` / `parse_cell` accept, so they
are found in *all* files — including the rejected ones, which often have both.

Usage:
    scripts/fesa_anomalies.py [--root test_files/fesa_results]
                              [--dump target/release/fesa-dump(.exe)]
                              [--out <root>/ANOMALIES.md] [--jobs N]
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

# --- rejections -------------------------------------------------------------

# Fold the variable parts of an error message so one group is one kind of defect.
NORMALIZERS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"^parsing [^:]*: "), ""),
    (re.compile(r"\bline \d+\b"), "line N"),
    (re.compile(r"\bround \d+\b"), "round N"),
    (re.compile(r"\(\d+ vs \d+\)"), "(N vs M)"),
    (re.compile(r"\bplayer number \d+\b"), "player number N"),
    (re.compile(r"\bplayer \d+\b"), "player N"),
    (re.compile(r"\bopponent \d+\b"), "opponent N"),
    (re.compile(r"\bhave \d+\b"), "have N"),
    (re.compile(r"\b\d+ and \d+\b"), "N and M"),
    (re.compile(r'"[^"]*"'), '"…"'),
]


def normalize(message: str) -> str:
    for pattern, replacement in NORMALIZERS:
        message = pattern.sub(replacement, message)
    return message.strip()


def variable_parts(message: str) -> str:
    """Just the bits [`normalize`] folded away — the line number, the offending
    cell, the round counts. The group heading already carries the rest, so this
    is what a per-file entry needs to add."""
    parts: list[str] = []
    for pattern, replacement in NORMALIZERS[1:]:  # [0] only strips the path
        parts += [m.group(0) for m in pattern.finditer(message)]
    return ", ".join(parts)


def check(dump: Path, path: Path, timeout: float) -> tuple[Path, str]:
    """(path, ""), or (path, error message) when the importer rejects the file."""
    try:
        p = subprocess.run(
            [str(dump), str(path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return path, "timeout"
    if p.returncode == 0:
        return path, ""
    lines = p.stderr.decode("utf-8", "replace").strip().splitlines()
    return path, (lines[-1] if lines else f"exit {p.returncode}")


# --- tolerated findings ------------------------------------------------------

# A data row: leading spaces, the player number, a space. Same test as the
# importer's `is_data_row`.
DATA_ROW = re.compile(r"^ *\d+ ")
# A parenthetical group, and whatever token it hangs off (empty if it opens the
# line or follows a space).
PAREN = re.compile(r"(\S*?)(\([^)]*\))")
# A round cell: an opponent number (0 for a bye/absence) and a result sign.
CELL = re.compile(r"^(\d+)[-+=#][\^v]?$")

BYE_HANDICAP = "Handicap mark on a bye or absence"
DETACHED = "Round-cell annotation separated from its cell"
IN_NAME = "Parenthetical inside the name columns"

TOLERATED: dict[str, str] = {
    BYE_HANDICAP: (
        "A cell with no opponent (`0+`, `0-`, `0=`, `0#`) carries a handicap "
        "annotation. No board was played, so the odds cannot describe anything; "
        "the importer validates the mark and then drops it."
    ),
    DETACHED: (
        "A `(…)` group sits a space away from the round cell it belongs to "
        "(`3+ (*)`), so nothing ties the two together. The importer only reads "
        "an annotation glued to its cell, and drops this one — whatever it meant "
        "about that game is lost."
    ),
    IN_NAME: (
        "A free-standing `(…)` group among the name columns, typically a "
        "disambiguating suffix on a first name (`Andriy (elder)`). It annotates "
        "no round cell, so the importer drops it and the player loses that part "
        "of their name."
    ),
}


def scan(path: Path) -> list[tuple[str, int, str]]:
    """(finding title, line number, the offending text) for one file.

    Classifies each parenthetical group in a data row by what it hangs off: glued
    to a played game's cell it is an ordinary handicap and no finding at all.
    """
    # The corpus is Latin-1, like every FESA export.
    text = path.read_text(encoding="latin-1")
    found: list[tuple[str, int, str]] = []
    for i, line in enumerate(text.splitlines(), start=1):
        # The first non-empty line is the title, never a data row; skipping
        # non-data rows keeps `(shogi)` in a tournament name out of the report.
        if not DATA_ROW.match(line):
            continue
        for m in PAREN.finditer(line):
            prefix, group = m.group(1), m.group(2)
            cell = CELL.match(prefix)
            if cell:
                # Glued to a cell: a real handicap, unless the cell is a bye or
                # an absence, which no odds can describe.
                if cell.group(1) == "0":
                    found.append((BYE_HANDICAP, i, prefix + group))
                continue
            if prefix:
                # Glued to something that is not a cell at all.
                found.append((IN_NAME, i, prefix + group))
                continue
            # Free-standing: a detached cell annotation if a cell precedes it.
            before = line[: m.start()].split()
            if before and CELL.match(before[-1]):
                found.append((DETACHED, i, f"{before[-1]} {group}"))
            else:
                found.append((IN_NAME, i, group))
    return found


# --- report ------------------------------------------------------------------


def render(
    root: Path,
    total: int,
    rejected: dict[str, list[tuple[Path, str]]],
    tolerated: dict[str, list[tuple[Path, int, str]]],
) -> str:
    out: list[str] = []
    out.append("# FESA corpus anomalies\n")
    out.append(
        f"Generated by `scripts/fesa_anomalies.py` over {total} files under "
        f"`{root}/`. Each entry is something the importer "
        f"(`crates/core/src/fesa_results.rs`) found wrong with the published "
        f"table — a list of things to report upstream to the FESA site.\n"
    )
    n_rejected = sum(len(v) for v in rejected.values())
    n_tolerated = len({p for v in tolerated.values() for p, _, _ in v})
    out.append(
        f"- **{n_rejected}** files are rejected outright "
        f"(`fesa_filter_valid.py` parks these in `invalid/`).\n"
        f"- **{n_tolerated}** files import, but only because the parser works "
        f"around something malformed and drops information doing it.\n"
    )

    out.append("\n## Rejected files\n")
    if not rejected:
        out.append("None.\n")
    for kind, entries in sorted(rejected.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        out.append(f"\n### {kind} — {len(entries)} file(s)\n")
        for path, message in sorted(entries):
            where = variable_parts(message)
            out.append(f"- `{path}`" + (f" — {where}" if where else ""))
        out.append("")

    out.append("\n## Tolerated anomalies\n")
    if not tolerated:
        out.append("None.\n")
    for title, description in TOLERATED.items():
        entries = tolerated.get(title)
        if not entries:
            continue
        by_file: dict[Path, list[tuple[int, str]]] = defaultdict(list)
        for path, line_no, text in entries:
            by_file[path].append((line_no, text))
        out.append(f"\n### {title} — {len(by_file)} file(s)\n")
        out.append(f"{description}\n")
        for path, hits in sorted(by_file.items()):
            shown = ", ".join(f"line {n}: `{t}`" for n, t in sorted(hits)[:4])
            more = f" (+{len(hits) - 4} more)" if len(hits) > 4 else ""
            out.append(f"- `{path}` — {shown}{more}")
        out.append("")

    return "\n".join(out) + "\n"


def find_dump(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    for cand in ("target/release/fesa-dump.exe", "target/release/fesa-dump"):
        if Path(cand).exists():
            return Path(cand)
    sys.exit(
        "fesa-dump binary not found; build it "
        "(cargo build --release --bin fesa-dump) or pass --dump"
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", default="test_files/fesa_results")
    ap.add_argument("--dump", default=None)
    ap.add_argument("--out", default=None)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--timeout", type=float, default=60.0)
    args = ap.parse_args()

    dump = find_dump(args.dump)
    root = Path(args.root)
    out_path = Path(args.out) if args.out else root / "ANOMALIES.md"

    # Every .txt under a season directory *and* under invalid/: a rejected file
    # is exactly what this report is for.
    files = sorted(root.glob("*/*.txt"))
    if not files:
        sys.exit(f"no result files under {root}/ (run fesa_fetch_all.py first)")
    print(f"checking {len(files)} files with {args.jobs} jobs …", file=sys.stderr)

    rejected: dict[str, list[tuple[Path, str]]] = defaultdict(list)
    tolerated: dict[str, list[tuple[Path, int, str]]] = defaultdict(list)
    done = 0
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        futs = [ex.submit(check, dump, f, args.timeout) for f in files]
        for fut in cf.as_completed(futs):
            path, message = fut.result()
            done += 1
            if message:
                rejected[normalize(message)].append((path.relative_to(root), message))
            if done % 500 == 0:
                print(f"  {done}/{len(files)}", file=sys.stderr)

    for f in files:
        for title, line_no, text in scan(f):
            tolerated[title].append((f.relative_to(root), line_no, text))

    out_path.write_text(render(root, len(files), rejected, tolerated), encoding="utf-8")
    n_rejected = sum(len(v) for v in rejected.values())
    n_tolerated = len({p for v in tolerated.values() for p, _, _ in v})
    print(
        f"wrote {out_path}: {n_rejected} rejected in {len(rejected)} group(s), "
        f"{n_tolerated} file(s) with tolerated anomalies",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
