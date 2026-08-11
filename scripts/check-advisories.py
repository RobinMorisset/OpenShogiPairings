#!/usr/bin/env python3
"""Security-advisory check for both of the repo's lockfiles.

`cargo audit` reads a Cargo.lock, and a lockfile is **feature-blind**: it records
a package's optional dependencies whether or not anything enables them. So it
reports advisories against crates that are never compiled into anything we ship —
today `rkyv` (optional in `rust_decimal`, feature off) and `bincode` (optional in
`typed-index-collections`, feature off). Neither can be upgraded either: their
fixes are semver-major moves their parent crate hasn't taken.

The usual answer is an ignore list. This runs the check instead, on every
invocation, by asking cargo which packages are *actually* in the resolved graph —
`cargo tree` is feature-aware, which is exactly what the lockfile isn't — and
keeping only the advisories whose package is really there. An ignore list is a
claim about the dependency graph on the day it was written, and it goes on
silently holding after it stops being true; this cannot.

**Why not `cargo tree -i <crate>` per advisory, keyed on its exit code:**

    cargo tree -i rkyv --target all             → exit 101 ("nothing to print")
    cargo tree -i no-such-crate-at-all          → exit 101

The same code means "not in the graph" and "that name matches nothing" — so a
script reading it as "phantom" would wave every advisory through the moment the
command broke for an unrelated reason (a typo, a manifest error, a cargo change).
A check that turns green when the tool fails is worse than no check. So the set
of compiled packages is computed *positively*, once per manifest, and anything
that goes wrong raises instead of being read as all-clear.

Matching is on name **and** version: with two majors of one crate in the graph
(`rand` 0.8 and 0.9, say), a by-name test can't tell which one is the affected
build.

Exit status: 1 if a real vulnerability is compiled in, 0 otherwise. Unmaintained
and unsound advisories are reported but don't fail the run, matching `cargo
audit`'s own default — they're a migration signal, not a break-the-build one.
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Every (manifest, lockfile) pair in the repo. `frontend/src-tauri` is
# deliberately outside the workspace (see the root Cargo.toml), so it carries its
# own lock with a dependency tree — Tauri, the webview stack — that the root lock
# never sees. Auditing only the root would leave the app the referees actually
# install as the one unchecked artifact.
TARGETS = [
    ("workspace", ROOT / "Cargo.toml", ROOT / "Cargo.lock"),
    (
        "desktop (src-tauri)",
        ROOT / "frontend" / "src-tauri" / "Cargo.toml",
        ROOT / "frontend" / "src-tauri" / "Cargo.lock",
    ),
]


def run(command):
    """Run a command, returning (exit status, stdout). Never raises on a non-zero
    status — callers decide, because `cargo audit` uses one to mean "found
    something" rather than "went wrong"."""
    result = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, check=False
    )
    if result.stderr:
        # cargo's progress and warnings — pass them through rather than hiding
        # them, so a fetch failure or a lockfile complaint is visible.
        sys.stderr.write(result.stderr)
    return result.returncode, result.stdout


def compiled_packages(manifest):
    """The `(name, version)` of every package in `manifest`'s resolved dependency
    graph, across all target platforms.

    Feature-aware, which is the whole point: an optional dependency nothing
    enables does not appear. `--target all` keeps the platform-specific halves in
    (the GTK stack a Linux build pulls in is real, even when this runs on macOS),
    and cargo's default edge set — normal, build and dev — keeps build scripts and
    test-only dependencies in scope too. Both choices err toward calling an
    advisory real, which is the safe direction for this check to be wrong in.
    """
    status, stdout = run(
        [
            "cargo",
            "tree",
            "--manifest-path",
            str(manifest),
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ]
    )
    if status != 0:
        raise RuntimeError(f"cargo tree failed for {manifest} (exit {status})")

    packages = set()
    for line in stdout.splitlines():
        # `name v1.2.3`, sometimes followed by ` (*)` (a subtree already shown),
        # ` (proc-macro)`, or the path of a local crate. Only the first two
        # fields matter.
        fields = line.split()
        if len(fields) >= 2 and fields[1].startswith("v"):
            packages.add((fields[0], fields[1][1:]))
    if not packages:
        raise RuntimeError(f"cargo tree listed no packages for {manifest}")
    return packages


def audit(lockfile):
    """`cargo audit`'s report for one lockfile, as parsed JSON.

    A non-zero exit means "advisories found", which is the ordinary case here, so
    the report itself is the success signal: unparseable output is what counts as
    a failure.
    """
    _, stdout = run(["cargo", "audit", "--json", "--file", str(lockfile)])
    try:
        return json.loads(stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"cargo audit produced no report for {lockfile}: {e}")


def entries(report):
    """Every advisory in a report as `(kind, advisory id, name, version, title)`,
    vulnerabilities first."""
    found = []
    for item in report["vulnerabilities"]["list"]:
        found.append(("vulnerability", item))
    for kind, items in report["warnings"].items():
        for item in items:
            found.append((kind, item))

    out = []
    for kind, item in found:
        advisory = item.get("advisory") or {}
        package = item["package"]
        out.append(
            (
                kind,
                advisory.get("id", "?"),
                package["name"],
                package["version"],
                advisory.get("title", ""),
            )
        )
    return out


def main():
    real_vulnerabilities = 0

    for label, manifest, lockfile in TARGETS:
        print(f"== {label} ==")
        packages = compiled_packages(manifest)
        report = audit(lockfile)

        dismissed = []
        informational = {}
        lines = 0
        for kind, advisory_id, name, version, title in entries(report):
            if (name, version) not in packages:
                dismissed.append(f"{advisory_id} ({name} {version})")
            elif kind == "vulnerability":
                print(f"  VULNERABLE: {name} {version} — {advisory_id}: {title}")
                real_vulnerabilities += 1
                lines += 1
            else:
                # Unmaintained and unsound advisories are real but not
                # actionable here — they come with the Tauri/GTK stack and clear
                # when it moves. One line per kind, because this runs on every
                # push and seventeen unchanged lines is how output stops being
                # read at all.
                informational.setdefault(kind, []).append(name)

        for kind, names in sorted(informational.items()):
            print(f"  {kind} ({len(names)}): {', '.join(sorted(set(names)))}")
            lines += 1
        if informational:
            print("  (informational — `cargo audit` has the advisory ids)")

        # Always printed, never just silently dropped: a dismissal is a claim
        # about the build graph, and it should be visible on the run that made it.
        if dismissed:
            print(
                f"  not compiled, so not applicable ({len(dismissed)}): "
                + ", ".join(dismissed)
            )
            lines += 1
        if lines == 0:
            print(f"  {len(packages)} packages, nothing to report")

    if real_vulnerabilities:
        print(
            f"\n{real_vulnerabilities} advisory/advisories affect code that is "
            f"actually built. Upgrade the dependency, or drop it."
        )
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as e:
        # The check could not be *performed*. That is a failure, not a pass: a
        # broken audit must never read as a clean one.
        print(f"advisory check failed to run: {e}", file=sys.stderr)
        sys.exit(2)
    except FileNotFoundError as e:
        print(
            f"advisory check failed to run: {e}\n"
            "cargo-audit is required: cargo install cargo-audit",
            file=sys.stderr,
        )
        sys.exit(2)
