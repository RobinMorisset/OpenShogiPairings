#!/usr/bin/env python3
"""Generate a fake FESA-format result file with N players and R rounds.

Built for benchmarking the pairing solver (`integer-blossom`) at sizes the real
corpus doesn't reach: the largest historical tournament is ~100 players, but the
matching is expected to scale ~N^3 and that is worth measuring directly. The
output is an ordinary FESA result table, so it feeds `osp-sim --results` exactly
like a real one:

    scripts/gen_fake_tournament.py 300 --rounds 7 --out big.txt
    target/release/osp-sim --results big.txt --runs 50   # times the solver at N=300

Realism comes from the sampling table `test_files/fesa_results/fesa_elo_pairs.json`
(built by `fesa_extract_elo_pairs.py` over the whole valid FESA corpus; it lives
under the gitignored corpus tree and travels in the same archive). Each synthetic
player is one real player drawn with replacement, carrying **together**:

- their pre-tournament **pairing ELO** (`start`, the rating the engine pairs on;
  `null` -> an unrated player, emitted as a `*` rating), and
- their post-tournament **true strength** (`end`, the oracle center osp-sim draws
  game outcomes from), encoded as the `+/-` delta `end - start` (or the `*`
  rating for an unrated player), so `osp-sim` recovers `end` exactly.

Keeping the pair joined preserves the real correlation (players end near where
they started) instead of pairing a beginner's rating with a master's strength.

**Why the base's own results don't matter.** `osp-sim` resets the base to
zero rounds and re-pairs + re-simulates every round itself — that is what
exercises the solver — so the only things it reads from this file are the player
ratings/strengths, the round count, and the *per-round attendance*. The game
outcomes in the cross-table are therefore a placeholder: trivially-consistent
pairings (lower number beats higher, one bye on an odd round). What is faithful
is the attendance: each player also carries their real per-round **absence
pattern** (`absent`). It is mapped onto R rounds by prefix, and where R runs past
the player's source event the tail *carries their final-round status forward* — a
player absent in their last real round is a genuine dropout and stays out, so the
present-set (the actual graph the solver matches each round) keeps shrinking round
to round as it really does, rather than snapping back to full attendance.

Usage:
    scripts/gen_fake_tournament.py N [--rounds R] [--out FILE] [--seed S]
        [--pairs scripts/fesa_elo_pairs.json] [--name TITLE]
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from pathlib import Path

# Small pools for plausible, ASCII/Latin-1-safe fake names. Uniqueness is not
# required (players are keyed by number); collisions are harmless.
_FIRST = (
    "Alex Bo Chen Dan Emi Finn Gaku Haru Ivan Jun Kai Lena Milo Nao Oskar "
    "Piotr Quentin Rin Sora Taro Uwe Vera Wei Xavier Yuki Zane Anna Erik "
    "Nina Leo Kaito Mei Sven Olga Raj Aiko Bjorn Clara Dima"
).split()
_LAST_A = (
    "Kob Tan Sato Nog Ber Lam Ike Kam Hay Oza Nguy Fern Van Pop Ivan Mor Sch "
    "Kli Wag Rot Bak Duf Chey Puc Mas Leit Wei Mil Kra Hor Fia Sen Nov"
).split()
_LAST_B = (
    "ashi aki eda ler kawa moto son berg enko ova sen mann inen escu ov ski "
    "eux her wicz stra ndez sson dahl insky arte hara uchi enko yama"
).split()
_NATS = "JP FR DE RU BE NL GB IT ES PL UA SE CZ HU AT CH FI".split()


def gen_name(rng: random.Random) -> tuple[str, str]:
    """A (last, first) pair of single alpha tokens (no internal double spaces, so
    the FESA last-name-column width detector reads a clean boundary)."""
    last = rng.choice(_LAST_A) + rng.choice(_LAST_B)
    return last, rng.choice(_FIRST)


class Player:
    __slots__ = ("number", "last", "first", "nat", "rating", "strength", "absent", "cells")

    def __init__(self, number, last, first, nat, rating, strength, absent):
        self.number = number
        self.last = last
        self.first = first
        self.nat = nat
        self.rating = rating  # int, or None for a pre-unrated player
        self.strength = strength  # int (the oracle center)
        self.absent = absent  # set of 1-based round numbers this player sits out
        self.cells: list[str] = []  # one per round, filled by build_cross_table


def map_absence(absent: list, src_rounds: int, rounds: int) -> set:
    """Map one real player's absence pattern (rounds they sat out of a
    `src_rounds`-round event) onto `rounds` rounds.

    For rounds within the source event it is a straight copy. Where `rounds` runs
    past the source (`rounds > src_rounds`), the tail carries the player's
    final-round status forward: a player absent in their last real round is a
    dropout and stays absent, otherwise they stay present. This keeps attendance
    decreasing over rounds — the realistic trend — instead of snapping back to
    full when a short-event player is placed in a longer one."""
    absent_set = set(absent)
    result = {r for r in absent_set if 1 <= r <= rounds}
    if rounds > src_rounds and src_rounds in absent_set:  # dropped out by the end
        result.update(range(src_rounds + 1, rounds + 1))
    return result


def sample_players(records: list, n: int, rounds: int, rng: random.Random) -> list[Player]:
    """Draw N real players with replacement, each carrying their joint (start,
    end) ratings and their absence pattern mapped onto `rounds` (see
    `map_absence`)."""
    players = []
    for i in range(n):
        start, end, src_rounds, absent = rng.choice(records)
        absent_here = map_absence(absent, src_rounds, rounds)
        last, first = gen_name(rng)
        players.append(
            Player(i + 1, last, first, rng.choice(_NATS), start, end, absent_here)
        )
    return players


def build_cross_table(players: list[Player], rounds: int, rng: random.Random) -> None:
    """Fill every player's per-round cells with a trivially-consistent placeholder
    schedule that honours each player's absence set.

    Present players are paired consecutively (after a per-round rotation so
    opponents vary); the lower-numbered side is recorded as the winner and the
    two cells mirror each other (`b+` / `a-`), which is exactly what the importer
    validates. An odd present-set gives one player a full-point bye (`0+`);
    absent players get `0#`. If a round would leave fewer than two present (only
    possible at tiny N), absentees are promoted back until two remain, since the
    round machinery needs a pairable pair."""
    n = len(players)
    by_number = {p.number: p for p in players}
    for p in players:
        p.cells = [None] * rounds

    for r in range(1, rounds + 1):
        present = [p for p in players if r not in p.absent]
        if len(present) < 2:
            # Degenerate round (tiny N): promote absentees until two are present.
            for p in players:
                if len(present) >= 2:
                    break
                if r in p.absent:
                    p.absent.discard(r)
                    present.append(p)

        present_numbers = sorted(p.number for p in present)
        # Rotate so the same field doesn't reproduce the same pairings each round.
        rot = (r - 1) % len(present_numbers)
        order = present_numbers[rot:] + present_numbers[:rot]

        bye_number = None
        if len(order) % 2 == 1:
            bye_number = order.pop()  # one player sits the bye this round

        for i in range(0, len(order), 2):
            a = min(order[i], order[i + 1])
            b = max(order[i], order[i + 1])
            by_number[a].cells[r - 1] = f"{b}+"  # lower number wins (placeholder)
            by_number[b].cells[r - 1] = f"{a}-"

        if bye_number is not None:
            by_number[bye_number].cells[r - 1] = "0+"

        for p in players:
            if r in p.absent:
                p.cells[r - 1] = "0#"

    # Sanity: every cell filled (no None left).
    for p in players:
        assert all(c is not None for c in p.cells), p.number


def points(cells: list[str]) -> int:
    """Placeholder point total from the cells: a win (`N+`) or a bye (`0+`) scores
    1; a loss or absence scores 0. Cosmetic — osp-sim recomputes standings."""
    return sum(1 for c in cells if c.endswith("+"))


def render(players: list[Player], rounds: int, title: str) -> str:
    """Render the players as a fixed-width FESA result table.

    Layout per row: `Nr  Last<pad>First Nat ELO[*] <cells…> Pts [+/-]`. Only the
    last name is positional; it is left-justified to (longest last name + 2) so
    every row has the >= 2-space gap the width detector keys on. A rated player
    carries the signed `+/-` delta (`end - start`), from which the importer
    recovers the true strength; an unrated player's ELO carries a `*` and no
    delta (the `*` rating already is their strength)."""
    last_w = max(len(p.last) for p in players) + 2
    nr_w = len(str(len(players)))

    header_cells = " ".join(f"R{i}" for i in range(1, rounds + 1))
    lines = [title, f"Nr Name Nat ELO {header_cells} Pts +/-"]
    for p in players:
        fields = [p.first, p.nat]
        if p.rating is None:
            fields.append(f"{p.strength}*")  # unrated: the * rating is the strength
            fields.extend(p.cells)
            fields.append(str(points(p.cells)))
        else:
            fields.append(str(p.rating))
            fields.extend(p.cells)
            fields.append(str(points(p.cells)))
            fields.append(f"{p.strength - p.rating:+d}")  # delta -> recovers strength
        line = f"{p.number:>{nr_w}} {p.last.ljust(last_w)}{' '.join(fields)}"
        lines.append(line.rstrip())
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("players", type=int, help="number of players N (>= 2)")
    ap.add_argument("--rounds", type=int, default=7, help="number of rounds R (default 7)")
    ap.add_argument("--out", default=None, help="output file (default: stdout)")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--pairs", default="test_files/fesa_results/fesa_elo_pairs.json")
    ap.add_argument("--name", default=None, help="tournament title line")
    args = ap.parse_args()

    if args.players < 2:
        sys.exit("need at least 2 players")
    if args.rounds < 1:
        sys.exit("need at least 1 round")

    records = json.loads(Path(args.pairs).read_text(encoding="utf-8"))["players"]
    if not records:
        sys.exit(f"{args.pairs} has no player records")

    rng = random.Random(args.seed)
    players = sample_players(records, args.players, args.rounds, rng)
    build_cross_table(players, args.rounds, rng)

    # A title that does not start with a digit (the importer takes line 1 as the
    # title unconditionally, but keep it unambiguous).
    title = args.name or f"Synthetic Benchmark N{args.players} R{args.rounds} : 2026-01-01"
    text = render(players, args.rounds, title)

    if args.out:
        # Latin-1, matching how osp-sim reads result files (decode_latin1).
        Path(args.out).write_bytes(text.encode("latin-1"))
        n_unrated = sum(1 for p in players if p.rating is None)
        print(
            f"wrote {args.players} players x {args.rounds} rounds "
            f"({n_unrated} unrated) to {args.out}",
            file=sys.stderr,
        )
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
