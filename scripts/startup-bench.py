#!/usr/bin/env python3
"""A/B benchmark for desktop app startup — see docs/desktop-startup-profiling.md.

Measures wall-clock time from `exec` to the moment the tournament picker has
actually *painted*, by listening for a one-shot HTTP ping that temporarily
instrumented frontend code fires after a double `requestAnimationFrame`. That
marker is the whole trick: it needs no platform-specific process inspection, so
the same harness runs on macOS, Windows and Linux.

Two modes, because they answer different questions:

  --mode warm   Each variant keeps a FIXED path, so after the warmups both have
                a warm executable (the OS's first-exec scan already paid) and a
                warm webview profile. This is "I've run this before".

  --mode cold   Every measurement gets a brand-new executable name, and the
                webview's per-app data directory is deleted afterwards. This is
                "I just built this and ran it for the first time".

Runs are INTERLEAVED in time-adjacent pairs with alternating order. This is not
optional: measuring the variants in separate blocks lets machine drift masquerade
as an effect, which is exactly how an early version of this investigation reached
a wrong conclusion. Interleaving also enables the paired test, which is markedly
more powerful here since consecutive runs share machine conditions.

Significance is assessed with permutation tests (no scipy dependency).

Examples:
    # A/B two builds, warm
    python3 scripts/startup-bench.py --a old.exe --b new.exe --mode warm

    # Characterise one build's cold launch
    python3 scripts/startup-bench.py --a app.exe --mode cold --pairs 10
"""

from __future__ import annotations

import argparse
import os
import platform
import random
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import uuid

DEFAULT_PORT = 47999


# --------------------------------------------------------------------------
# Where each platform's webview keeps its per-app profile.
#
# Cold runs must delete this, otherwise the second run of a given name reuses a
# warm code cache and populated localStorage and is no longer "cold".
#
# macOS is verified: with no Info.plist (a `--no-bundle` build) WKWebView keys
# the store on the EXECUTABLE NAME, so a renamed copy transparently gets a fresh
# one. The Windows and Linux paths are best-effort guesses — CHECK THEM against
# your actual build before trusting a cold number, and override with
# --purge-glob if they are wrong.
# --------------------------------------------------------------------------
def webview_data_dirs(exe_path: str, name: str) -> list[str]:
    system = platform.system()
    home = os.path.expanduser("~")
    if system == "Darwin":
        return [os.path.join(home, "Library", "WebKit", name)]
    if system == "Windows":
        local = os.environ.get("LOCALAPPDATA", os.path.join(home, "AppData", "Local"))
        return [
            # WebView2's default when the host doesn't pick one.
            os.path.join(os.path.dirname(os.path.abspath(exe_path)), name + ".WebView2"),
            # What Tauri/wry typically set instead.
            os.path.join(local, name, "EBWebView"),
        ]
    return [os.path.join(home, ".local", "share", name)]


class Marker:
    """Listens for the one-shot 'picker painted' ping from the instrumented UI."""

    def __init__(self, port: int):
        self.srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", port))
        self.srv.listen(8)

    def drain(self) -> None:
        """Discard late pings from a previous run so they can't be miscounted."""
        self.srv.settimeout(0.01)
        while True:
            try:
                self.srv.accept()[0].close()
            except Exception:
                return

    def wait(self, timeout: float) -> float | None:
        self.srv.settimeout(timeout)
        try:
            conn, _ = self.srv.accept()
        except socket.timeout:
            return None
        stamp = time.time()
        try:
            conn.recv(256)
            conn.sendall(
                b"HTTP/1.1 204 No Content\r\n"
                b"Access-Control-Allow-Origin: *\r\n"
                b"Content-Length: 0\r\n\r\n"
            )
        except Exception:
            pass
        conn.close()
        return stamp

    def close(self) -> None:
        self.srv.close()


def launch_and_time(exe: str, marker: Marker, timeout: float, settle: float) -> float | None:
    """One launch. Returns seconds from spawn to 'picker painted', or None."""
    marker.drain()
    proc = subprocess.Popen(exe, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.time()
    stamp = marker.wait(timeout)

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()
    # Let the webview's helper processes exit, so the next run starts clean.
    time.sleep(settle)
    return None if stamp is None else stamp - t0


def cold_run(src: str, marker: Marker, workdir: str, args) -> float | None:
    """Launch `src` under a never-before-seen name, then erase every trace."""
    name = "ospbench" + uuid.uuid4().hex[:10]
    if platform.system() == "Windows":
        name += ".exe"
    path = os.path.join(workdir, name)
    shutil.copy2(src, path)  # a fresh file => pays the OS first-exec scan
    if platform.system() != "Windows":
        os.chmod(path, 0o755)

    stem = name[:-4] if name.endswith(".exe") else name
    try:
        return launch_and_time(path, marker, args.timeout, args.settle)
    finally:
        for d in webview_data_dirs(path, stem):
            shutil.rmtree(d, ignore_errors=True)
        try:
            os.remove(path)
        except OSError:
            pass


def perm_unpaired(x: list[float], y: list[float], iters: int = 20000) -> tuple[float, float]:
    """Two-sided permutation test on the difference of medians."""
    obs = statistics.median(x) - statistics.median(y)
    pool = list(x) + list(y)
    n = len(x)
    hits = 0
    for _ in range(iters):
        random.shuffle(pool)
        if abs(statistics.median(pool[:n]) - statistics.median(pool[n:])) >= abs(obs):
            hits += 1
    return obs, hits / iters


def perm_paired(diffs: list[float], iters: int = 20000) -> tuple[float, float]:
    """Sign-flip permutation test on paired differences."""
    obs = statistics.mean(diffs)
    hits = 0
    for _ in range(iters):
        flipped = [d if random.random() < 0.5 else -d for d in diffs]
        if abs(statistics.mean(flipped)) >= abs(obs):
            hits += 1
    return obs, hits / iters


def describe(label: str, s: list[float]) -> None:
    if not s:
        print(f"  {label:22s} NO SAMPLES")
        return
    sd = statistics.pstdev(s) if len(s) > 1 else 0.0
    print(
        f"  {label:22s} n={len(s):2d}  median={statistics.median(s):.3f}s  "
        f"mean={statistics.mean(s):.3f}s  sd={sd:.3f}  "
        f"min={min(s):.3f}  max={max(s):.3f}"
    )


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--a", required=True, help="binary for variant A (the baseline)")
    p.add_argument("--b", help="binary for variant B; omit to characterise A alone")
    p.add_argument("--label-a", default="A")
    p.add_argument("--label-b", default="B")
    p.add_argument("--mode", choices=("warm", "cold"), default="warm")
    p.add_argument("--pairs", type=int, default=15, help="measurements per variant")
    p.add_argument("--warmups", type=int, default=3, help="discarded runs (warm mode only)")
    p.add_argument("--port", type=int, default=DEFAULT_PORT, help="marker port")
    p.add_argument("--timeout", type=float, default=30.0, help="seconds to wait for the paint ping")
    p.add_argument("--settle", type=float, default=0.7, help="pause between runs")
    args = p.parse_args()

    for path in filter(None, (args.a, args.b)):
        if not os.path.exists(path):
            print(f"error: no such binary: {path}", file=sys.stderr)
            return 2

    marker = Marker(args.port)
    workdir = tempfile.mkdtemp(prefix="osp-startup-bench-")
    variants = [(args.a, args.label_a)]
    if args.b:
        variants.append((args.b, args.label_b))

    # In warm mode each variant gets its own fixed path, so its executable and
    # webview profile stay warm across the whole session.
    warm_paths: dict[str, str] = {}
    if args.mode == "warm":
        for src, label in variants:
            ext = ".exe" if platform.system() == "Windows" else ""
            dst = os.path.join(workdir, f"warm_{label}{ext}")
            shutil.copy2(src, dst)
            if platform.system() != "Windows":
                os.chmod(dst, 0o755)
            warm_paths[label] = dst

        print(f"warming ({args.warmups} discarded runs each)...", flush=True)
        for _ in range(args.warmups):
            for _src, label in variants:
                launch_and_time(warm_paths[label], marker, args.timeout, args.settle)

    samples: dict[str, list[float]] = {label: [] for _s, label in variants}
    diffs: list[float] = []

    print(f"\n{args.mode} mode, {args.pairs} measurements per variant, interleaved\n", flush=True)
    for i in range(args.pairs):
        order = list(variants)
        if i % 2:  # alternate order to cancel any ordering bias
            order.reverse()
        got: dict[str, float | None] = {}
        for src, label in order:
            if args.mode == "warm":
                got[label] = launch_and_time(warm_paths[label], marker, args.timeout, args.settle)
            else:
                got[label] = cold_run(src, marker, workdir, args)

        parts = []
        for _s, label in variants:
            v = got[label]
            if v is not None:
                samples[label].append(v)
            parts.append(f"{label}={v:.3f}s" if v is not None else f"{label}=TIMEOUT")
        if args.b and got[args.label_a] is not None and got[args.label_b] is not None:
            d = got[args.label_a] - got[args.label_b]
            diffs.append(d)
            parts.append(f"diff={d:+.3f}s")
        print(f"  {i + 1:2d}: " + "  ".join(parts), flush=True)

    print(f"\n=== {args.mode.upper()} RESULTS ===")
    for _s, label in variants:
        describe(label, samples[label])

    a_s, b_s = samples[args.label_a], samples.get(args.label_b, [])
    if args.b and len(a_s) > 2 and len(b_s) > 2:
        obs_u, p_u = perm_unpaired(a_s, b_s)
        print(f"\n  UNPAIRED  median({args.label_a}) - median({args.label_b}) = {obs_u:+.3f}s   p = {p_u:.4f}")
        if diffs:
            obs_p, p_p = perm_paired(diffs)
            wins = sum(1 for d in diffs if d > 0)
            print(f"  PAIRED    mean(diff) = {obs_p:+.3f}s   p = {p_p:.4f}")
            print(f"            {args.label_b} faster in {wins}/{len(diffs)}")
            best = min(p_u, p_p)
        else:
            best = p_u
        print(f"\n  => {'SIGNIFICANT at 0.05' if best < 0.05 else 'NOT significant'}")
        # A null result on a noisy cold run usually means underpowered, not equal.
        pooled = statistics.pstdev(a_s + b_s)
        if best >= 0.05 and pooled > 0.03:
            print(f"     (noise sd={pooled:.3f}s is large — a small effect could be hiding;")
            print(f"      treat this as 'no evidence', not 'no difference')")

    marker.close()
    shutil.rmtree(workdir, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
