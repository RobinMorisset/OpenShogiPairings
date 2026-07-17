# Desktop startup profiling

Why the packaged desktop app takes a beat to appear, where that time actually
goes, and how to measure it reliably enough to tell a real speedup from noise.
The numbers below were gathered on **macOS**; the point of writing this down is
so the same investigation can be repeated on **Windows** and the two compared,
since the app is a Tauri build and the bottleneck turned out to be the native
side, which differs completely between platforms.

- **Environment measured:** macOS 26.5.2 (build 25F84), Apple Silicon, Tauri
  2.11, `--no-bundle` release build, commit `e39ff12`.
- **Harness:** [`scripts/startup-bench.py`](../scripts/startup-bench.py) — runs
  as-is on macOS, Windows and Linux.

## TL;DR of the macOS findings

- **Warm launch to the tournament picker painting is ~0.49s.** A first-ever
  launch of a freshly built binary is **~1.1–1.4s** — 2.5–3× slower — and every
  launch after is fast. That one-time cost is what makes it "slow the first
  time, fine afterwards".
- **The first-launch penalty is the OS vetting the binary**, not our code. It is
  keyed to the executable file (a new inode), so a rebuild pays it again, once.
  It lands entirely *before* `main()` runs (measured: 599ms to `main` cold vs
  ~20ms warm).
- **Rust startup is ~20ms.** Loading 533 dylibs and reaching `main()` is cheap;
  the embedded server's TCP bind is 0.3ms. There is no JIT, GC, or parsing cost
  to speak of — the intuition that a statically compiled binary should start
  fast is correct.
- **The ~0.25s before the UI reacts is native macOS framework init** — building
  the `NSWindow` and the `WKWebView` inside AppKit's
  `applicationDidFinishLaunching`. None of our code is in that hot path, and it
  is not something a Rust change can move.
- The remaining ~0.2s (of the 0.49s) is webview boot + JS parse + first render.

None of this is a bug to fix; it is a map of where an inherently native cost
lives, so effort isn't wasted optimising the ~20ms that is actually ours.

## The one change that came out of it

Startup blocked mounting the UI on the locale catalogue, and only *then* did the
first component resolve the API base (a dynamic import plus an IPC round-trip).
Nothing about locating the server needs the locale, so commit `e39ff12` kicks
that off in parallel from [`main.ts`](../frontend/src/main.ts) via
`prewarmApiBase`. Worth a measured **~17ms (~3.5%)**, `p ≈ 0.005`. Small, but it
is the only part of the warm path that is ours to move, and it is the worked
example for the methodology below.

## Methodology (read this before trusting any number)

Two mistakes produced wrong conclusions during this investigation. Both are easy
to make and easy to avoid.

### 1. Measure "picker painted", not "first request"

The tempting metric — time until the app makes its first HTTP call to the
embedded server — is worse than useless for judging a startup change, because
moving work earlier improves *that number by construction* while the picture on
screen appears at the same instant. You end up "proving" a speedup that the user
never sees.

The harness instead measures time to the **picker actually painting**. It does
this with a temporary instrumentation line in the frontend that pings a local
listener after a double `requestAnimationFrame` (i.e. after the browser has
committed a frame). The double-rAF matters: a single one fires before paint.

This marker is also what makes the harness **cross-platform** — it rides on the
webview, so it needs no `lsof`, no `/proc`, no Win32 process APIs.

### 2. Interleave the variants; never run them in separate blocks

The first warm A/B here ran all of variant A, then all of variant B, n=8 each.
It reported "no significant difference" — wrong. Two independent problems:

- **Blocked runs confound drift with effect.** Anything that changed between the
  two blocks (thermal state, background load) is indistinguishable from the
  thing you're testing.
- **n=8 was underpowered** for a ~17ms effect sitting on ~30ms of noise.

The fix, which the harness enforces: run the variants in **time-adjacent pairs**
with **alternating order** (A,B then B,A then A,B…). Drift now hits both
variants equally, and adjacent pairs share machine conditions, which also lets
you use the **paired** test (more powerful than the unpaired one). Interleaving
alone cut the baseline noise from sd≈0.030s to sd≈0.015s, which is what made the
17ms effect visible at `p ≈ 0.005` with n=15.

### 3. A null result on a noisy run means "no evidence", not "no difference"

The cold A/B here returned `p = 0.77` — but cold noise has sd≈0.06–0.14s, so
detecting 17ms would need ~200 samples per arm, not 15. That is *underpowered*,
not *equal*. The harness prints a warning when it reports non-significance on top
of large noise. Don't over-read it.

## Running the benchmark

The harness needs the frontend temporarily instrumented to fire the paint ping.

### Step 1 — add the paint marker (temporary)

In [`frontend/src/lib/components/TournamentPicker.svelte`](../frontend/src/lib/components/TournamentPicker.svelte),
right after the `void refresh();` line at the end of the `<script>` block:

```svelte
  // ==== TEMPORARY BENCHMARK INSTRUMENTATION — DELETE BEFORE COMMIT ====
  let benchMarked = false;
  $effect(() => {
    if (loading || benchMarked) return;
    benchMarked = true;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        void fetch("http://127.0.0.1:47999/painted").catch(() => {});
      }),
    );
  });
  // ==== END TEMPORARY BENCHMARK INSTRUMENTATION ====
```

This pings the harness once, the moment the picker's tournament list has
painted. It is deliberately not committed — the marker only exists while
benchmarking.

### Step 2 — build the variant(s)

Build each binary you want to compare **with the marker in place**, so both
sides are measured identically. To A/B a source change, build once with the
change and once without it (e.g. `git stash`), keeping the marker in both.

```sh
cd frontend
npm run tauri build -- --no-bundle
# copy the produced binary somewhere stable before building the next variant:
#   src-tauri/target/release/OpenShogiPairings           (macOS/Linux)
#   src-tauri/target/release/OpenShogiPairings.exe        (Windows)
```

### Step 3 — run

```sh
# Warm A/B of two builds (the common case):
python3 scripts/startup-bench.py \
    --a /path/to/baseline --label-a baseline \
    --b /path/to/changed  --label-b changed  \
    --mode warm --pairs 15

# Cold-launch characterisation of a single build:
python3 scripts/startup-bench.py --a /path/to/app --mode cold --pairs 15
```

- **`--mode warm`** — fixed path per variant, warmups discarded. Answers "does
  this change help a normal, repeat launch?" Low noise; use for small effects.
- **`--mode cold`** — fresh executable name + purged webview profile each run.
  Answers "how slow is the very first launch after a build?" High noise; needs
  many samples to compare variants.

### Step 4 — clean up

Delete the instrumentation block. Confirm nothing is left:

```sh
git checkout -- frontend/src/lib/components/TournamentPicker.svelte
grep -rn "47999\|BENCHMARK INSTRUMENTATION" frontend/src/   # expect no matches
```

## Deeper profiling: where the native time goes

The harness only gives one wall-clock number. To decompose it, two techniques
were used and are worth repeating on Windows.

### Phase timeline from inside the process

Temporary `eprintln!` marks were added at `main()` entry, `run()` entry,
`setup()` entry, and after the TCP bind, each printing ms elapsed since an
`OSP_T0` env var set immediately before `exec`. Everything before `main()` is
`exec` + the dynamic loader and cannot be touched by our code. The warm macOS
timeline:

| mark                        | elapsed | what it is                                   |
| --------------------------- | ------- | -------------------------------------------- |
| `main()` entered            | ~20 ms  | `exec` + dyld loading 533 dylibs             |
| `run()` entered             | ~20 ms  | our code starts                              |
| `setup()` entered           | ~255 ms | **the gap is AppKit + WebKit window init**   |
| port bound (server ready)   | ~255 ms | TCP bind: 0.3 ms                             |

The entire quarter-second is the gap between `run()` and `setup()`, which is
inside `tauri::Builder::run()` — i.e. framework work, before any of our setup
logic runs.

### Sampling profiler

macOS `sample` (attach while the process starts) showed that gap is spent under
`-[NSApplication run]` → `_sendFinishLaunchingNotification` → a callout back into
the binary where Tauri/wry create the window and webview. Symbolicated leaves:
`NSWindow`/`NSView` construction, title-bar button building
(`_NSThemeWidget initWithButtonID`), WindowServer round-trips (SkyLight
`SLSGetDisplaysWithUUID`), ObjC class introspection, and CFBundle/CFPreferences
lookups. All of it is the irreducible cost of asking macOS for a native window
containing a `WKWebView`.

Windows equivalents:
- **Timeline:** the same `eprintln!`-since-`exec` marks work unchanged.
- **Profiler:** use **WPR/WPA** (Windows Performance Recorder/Analyzer) or
  Visual Studio's sampling profiler instead of `sample`. Expect the native cost
  to be **WebView2 / `CoreWebView2Environment` creation and the Win32 window**,
  not AppKit — this is the most interesting thing to compare between platforms.

### Hypotheses that were tested and *refuted* (don't re-chase on macOS)

Recording these because each looked plausible and cost time to rule out:

- **Gatekeeper caching by code-signing hash.** A copy re-signed to a fresh
  cdhash was *not* slower than one with an identical cdhash — the first-launch
  cost is keyed to the file/inode, not the signature.
- **WebKit data-store creation.** A fresh inode whose webview profile already
  existed was still slow → the cost is the inode, not the store.
- **The FESA rating-list download** (`crates/server/src/ratings.rs` fetches once
  on first-ever run). Real, but its cache predated the launches under test, so
  it was not the cause of the slow launches actually observed.
- **The missing `Info.plist` in `--no-bundle`.** A properly bundled `.app` (real
  `Info.plist` + bundle id) was ~10–20ms *slower*, not faster. The
  CFBundle/CFPreferences frames are normal AppKit chatter, not a `--no-bundle`
  artifact.

## Windows notes (fill in when you run it)

- **Purge paths.** `scripts/startup-bench.py` deletes the webview profile between
  cold runs; the Windows/WebView2 paths in `webview_data_dirs()` are a
  best-effort guess. **Verify them against your actual build** before trusting a
  cold number — after one cold run, check whether a `*.WebView2` /`EBWebView`
  folder was created and adjust if needed.
- **`.exe` naming.** The harness appends `.exe` to its throwaway copies on
  Windows automatically.
- **Expect a different split.** The ~20ms loader + ~0.25s AppKit split is
  macOS-specific. On Windows the loader cost and especially the WebView2 init
  cost may differ substantially — that comparison is the whole reason this doc
  exists. Record the phase timeline and a WPA capture alongside the macOS
  numbers above.
