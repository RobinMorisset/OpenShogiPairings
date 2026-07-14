#!/usr/bin/env python3
"""Split a FESA "Tournament Results" HTML page into per-tournament result files.

FESA publishes each season's results at URLs like

    https://fesashogi.eu/old/index.php?mid=4&dateid=Spring+2026&tournamentid=all

as a run of HTML <table>s — one per tournament — carrying the same cross-table
the repo already understands from the fixed-width text export (see
`crates/core/src/fesa_results.rs`). This script turns that one page into N text
files, each in the exact result-table layout `import_fesa_results` parses, so
they can be fed straight to `osp-sim --results <file>`.

Each tournament table looks like:

    <tr><td colspan=12 ...><h3>TITLE<br>DATE</h3></td></tr>          # title
    <tr><th>Nr</th><th colspan=2>Name</th>...<th>Pts</th><th>+/-</th></tr>
    <tr><td>1</td><td><a>Last</a></td><td><a>First</a></td>          # player
        <td>Nat</td><td>Grade</td><td>ELO</td>
        <td>2+</td>...<td>Pts</td><td>+/-</td></tr>
    ...
    <tr><td colspan=12 ...>Promoting ...</td></tr>                   # ignored

We keep round cells verbatim (including parenthetical forfeit/handicap
annotations like `2-(+6p)` — the Rust parser strips those itself) and reproduce
the fixed-width last-name column the parser detects per file. The grade and the
`+/-` delta are absent for a player who was unrated before the tournament (their
ELO then carries a trailing `*`), matching the text format.

Output is written as Latin-1, since the simulator reads result files with
`decode_latin1` and the FESA page itself is ISO-8859-1.

Usage:
    scripts/fesa_html_to_results.py PAGE.html [--outdir DIR]
    scripts/fesa_html_to_results.py <URL>    [--outdir DIR]   # fetches the page
"""

from __future__ import annotations

import argparse
import html
import os
import re
import sys
import unicodedata
import urllib.request

# One tournament per <table>. The page has nothing but these result tables in
# its content area, so a non-greedy table sweep is enough.
TABLE_RE = re.compile(r"<table\b.*?</table>", re.S | re.I)
ROW_RE = re.compile(r"<tr\b.*?</tr>", re.S | re.I)
TD_RE = re.compile(r"<td\b[^>]*>(.*?)</td>", re.S | re.I)
H3_RE = re.compile(r"<h3\b[^>]*>(.*?)</h3>", re.S | re.I)
TAG_RE = re.compile(r"<[^>]+>")


def cell_text(raw: str) -> str:
    """Strip inner tags (e.g. the <a> profile links), unescape entities, and
    normalise the no-break spaces FESA uses for empty cells to plain blanks."""
    text = TAG_RE.sub("", raw)
    text = html.unescape(text)
    return text.replace("\xa0", " ").strip()


def title_from_table(table: str) -> str | None:
    """The tournament's title line: the <h3> text with its `<br>` between name
    and date turned into the ` : ` separator the text format uses."""
    m = H3_RE.search(table)
    if not m:
        return None
    inner = re.sub(r"<br\s*/?>", " : ", m.group(1), flags=re.I)
    title = cell_text(inner)
    # Collapse internal whitespace so a stray newline in the source doesn't
    # split the title across lines (the parser wants it on one line).
    return re.sub(r"\s+", " ", title)


def player_rows(table: str):
    """Yield each player row as a list of cell strings. A player row is a <tr>
    whose first <td> is a bare number; title, header, blank and "Promoting …"
    rows use `colspan` or `<th>` and are skipped."""
    for tr in ROW_RE.findall(table):
        if "colspan" in tr.lower() or "<th" in tr.lower():
            continue
        cells = [cell_text(td) for td in TD_RE.findall(tr)]
        if cells and cells[0].isdigit():
            yield cells


def is_rated(cells: list[str]) -> bool:
    """A player rated before the tournament: their ELO cell (always the 6th, ahead
    of any round-column drift) has no `*` suffix. Pre-unrated players carry `*` and
    no `+/-`."""
    return not cells[5].endswith("*")


def round_count(rows: list[list[str]]) -> int:
    """Rounds per game, taken from the *widest* row rather than the HTML header.

    Some FESA tables are mis-generated with one round missing from their <th>
    header: every player's last round then spills rightward, and a rated player's
    `+/-` lands in an extra cell past the declared columns. The player <td>s are
    still complete, so the true width is the data's, not the header's. A rated row
    is always `Nr, Last, First, Nat, Grade, ELO` (6) + rounds + `Pts, +/-` (2), so
    `rounds = width - 8`; we read it off rated rows (unrated rows are unreliable —
    a well-formed table pads their empty `+/-` with a cell, a mis-generated one
    drops it). Falls back to the overall widest row if a table has no rated player.
    """
    rated_widths = [len(r) for r in rows if is_rated(r)]
    if rated_widths:
        return max(rated_widths) - 8
    return max((len(r) for r in rows), default=8) - 8


def format_tournament(title: str, rows: list[list[str]]) -> str:
    """Render one tournament as a fixed-width result table.

    Layout per row: `Nr  Last<pad>First Nat [Grade] ELO <cells…> Pts [+/-]`.
    Only the last name is positional — the parser detects its column width and
    tokenises the rest — so we left-justify it to (longest last name + 2), which
    guarantees the >= 2 space gap the width detector keys on. Everything else is
    single-space separated.
    """
    last_width = max((len(r[1]) for r in rows), default=0) + 2
    nr_width = max((len(r[0]) for r in rows), default=1)
    nrounds = round_count(rows)

    lines = [title, "Nr Name Nat Grade ELO ... Pts +/-"]
    for cells in rows:
        # cells: Nr, Last, First, Nat, Grade, ELO, <nrounds cells…>, Pts, [+/-].
        # Slice by nrounds rather than trusting each row's own width, so a
        # mis-generated table's short (no-`+/-`) unrated rows still line up.
        nr, last, first, nat, grade, elo = cells[:6]
        rounds = cells[6 : 6 + nrounds]
        trailing = cells[6 + nrounds :]  # Pts, then an optional +/- delta
        pts = trailing[0] if trailing else "0"
        delta = next((t for t in trailing[1:] if t), "")  # skip an empty (nbsp) cell

        fields = [first, nat]
        if grade:  # absent for a pre-unrated player
            fields.append(grade)
        fields.append(elo)  # keeps the trailing `*` for pre-unrated players
        fields.extend(rounds)
        fields.append(pts)
        if delta:  # absent (nbsp) for a pre-unrated player
            fields.append(delta)

        line = f"{nr:>{nr_width}} {last.ljust(last_width)}{' '.join(fields)}"
        lines.append(line.rstrip())
    return "\n".join(lines) + "\n"


def slugify(title: str, index: int) -> str:
    """A filesystem-safe, unique file name from the title, e.g.
    `01_torneo_de_otono_2025_barcelona_spain_2025-11-29.txt`. The leading index
    keeps names unique even if two tournaments share a title."""
    ascii_title = (
        unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode("ascii")
    )
    slug = re.sub(r"[^a-zA-Z0-9]+", "_", ascii_title).strip("_").lower()
    slug = slug[:80].strip("_") or "tournament"
    return f"{index:02d}_{slug}.txt"


def load_page(source: str) -> str:
    """Read the page from a local file, or fetch it if `source` is a URL. FESA
    serves ISO-8859-1, so decode as Latin-1 either way."""
    if source.startswith(("http://", "https://")):
        with urllib.request.urlopen(source) as resp:
            return resp.read().decode("latin-1")
    with open(source, "rb") as f:
        return f.read().decode("latin-1")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("source", help="path to a saved FESA results page, or its URL")
    ap.add_argument("--outdir", default="fesa_results", help="directory for the generated result files (default: ./fesa_results)")
    args = ap.parse_args()

    doc = load_page(args.source)
    os.makedirs(args.outdir, exist_ok=True)

    written = 0
    skipped = 0
    for table in TABLE_RE.findall(doc):
        title = title_from_table(table)
        rows = list(player_rows(table))
        if title is None or not rows:
            skipped += 1
            continue
        written += 1
        path = os.path.join(args.outdir, slugify(title, written))
        with open(path, "wb") as f:
            f.write(format_tournament(title, rows).encode("latin-1"))
        print(f"{path}  ({len(rows)} players)")

    print(f"\nWrote {written} result files to {args.outdir}/"
          + (f" ({skipped} tables skipped)" if skipped else ""), file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
