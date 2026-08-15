# Desktop startup profiling

Why the packaged desktop app takes a beat to appear, where that time actually
goes, and how to measure it reliably enough to tell a real speedup from noise.
The numbers below were gathered on **macOS**; the point of writing this down was
so the same investigation could be repeated on **Windows** and the two compared,
since the app is a Tauri build and the bottleneck turned out to be the native
side, which differs completely between platforms. That Windows run has now been
done — see [Windows results](#windows-results) at the end.

- **Environment measured:** macOS 26.5.2 (build 25F84), Apple Silicon, Tauri
  2.11, `--no-bundle` release build, commit `e39ff12`.
- **Harness:** [`scripts/startup-bench.py`](../../scripts/startup-bench.py) — runs
  as-is on macOS and Linux. On Windows the **warm** mode runs as-is, but **cold**
  mode needs two fixes first (see [Windows results](#windows-results)): the
  webview purge path is wrong, and the cold trick doesn't reproduce the Windows
  first-launch penalty at all.

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
that off in parallel from [`main.ts`](../../frontend/src/main.ts) via
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

In [`frontend/src/lib/components/TournamentPicker.svelte`](../../frontend/src/lib/components/TournamentPicker.svelte),
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

## Windows results

Repeated on **Windows 10 Home 22H2 (build 19045)**, x64, Tauri 2.11,
`--no-bundle` release build, commit `fd6d7fb`, same
[`scripts/startup-bench.py`](../../scripts/startup-bench.py). Windows Defender
real-time protection **on**. The short version: **the shape matches macOS — a
fast warm launch dominated by native window init, plus a large one-time
first-launch penalty that lands entirely before `main()` — but every number is
smaller, and the first-launch penalty is keyed differently (see below), which
made the stock harness under-report it.**

### The numbers

| metric                          | Windows           | macOS (above)     |
| ------------------------------- | ----------------- | ----------------- |
| Warm launch → picker painted    | **0.382 s** (sd 0.005, n=15) | ~0.49 s |
| First-ever launch → painted     | **~0.82 s** (~2.1× warm)     | ~1.1–1.4 s (2.5–3×) |
| `exec` → `main()` **warm**      | **~8 ms**         | ~20 ms            |
| `exec` → `main()` **cold**      | **~370–630 ms**   | ~599 ms           |
| `run()` → `setup()` gap (warm)  | **~288 ms**       | ~235 ms           |
| TCP bind (`setup` → `bound`)    | **~2.9 ms**       | ~0.3 ms           |

Warm phase timeline (file-based `mark()` marks, medians, n=8):

| mark    | elapsed | what it is                                        |
| ------- | ------- | ------------------------------------------------- |
| `main`  | ~7.8 ms | `exec` + the loader reaching `main()`             |
| `run`   | +0.4 ms | our code starts                                   |
| `setup` | ~293 ms | **the gap is WebView2 + Win32 window init**       |
| `bound` | +2.9 ms | TCP bind                                           |
| painted | ~389 ms | + webview boot + JS parse + first render (~90 ms) |

Cold (first exec of a freshly built binary): `main` ~373 ms, `bound` ~710 ms,
painted ~821 ms — i.e. ~365 ms of the ~430 ms cold penalty is spent *before
`main()`*, exactly as on macOS.

### What matches macOS and what differs

- **Same overall shape.** Warm is fast; the quarter-second before the UI reacts
  is native window init inside `tauri::Builder::run()` (here **WebView2 +
  Win32**, there AppKit + WebKit); a large one-time first-launch penalty sits
  almost entirely before `main()` and is the OS vetting a new executable, not
  our code. None of it is a bug.
- **Everything is a bit faster on Windows.** Warm paint 0.38 s vs 0.49 s;
  `exec`→`main()` ~8 ms vs ~20 ms (fewer/faster DLL loads than macOS's 533
  dylibs); first launch ~0.82 s vs ~1.1–1.4 s.
- **The native window-init gap is slightly larger** (~288 ms vs ~235 ms) and is
  still the dominant warm cost and the only thing worth caring about — and it is
  WebView2/Win32 framework work, not ours.

### Two methodology traps specific to Windows (both cost time here)

1. **`--mode cold` does *not* reproduce the first-launch penalty on Windows.**
   The macOS penalty is keyed to the file/inode, so the harness's "copy to a
   fresh name" defeats the OS cache and each cold run is genuinely cold. On
   Windows the penalty is **Defender scanning novel executable *content*** — a
   byte-identical copy under a new name hits Defender's cache and launches warm.
   So `--mode cold` reported ~0.41 s (barely above warm) and looked like "no
   first-launch cost", which is wrong. **To see the real Windows first-launch
   cost you must launch a genuinely new build's binary directly** (novel
   content), once — which is what the phase-timeline warmup / a first-exec probe
   do, giving the ~0.82 s above. A rebuild with any code change pays it once;
   identical bytes never pay it again.

2. **`webview_data_dirs()`'s Windows guesses purge nothing.** WebView2 keys its
   user-data folder on the **app identifier**, not the exe name:
   `%LOCALAPPDATA%\org.openshogipairings.desktop\EBWebView` (verified — tournament
   data lives separately under `%APPDATA%\openshogipairings\tournaments`, so
   purging the webview folder is safe). The harness's `<name>.WebView2` /
   `<name>\EBWebView` guesses never match, so a renamed cold copy silently reuses
   a warm webview profile. With the correct folder purged, a cold webview adds
   only ~30 ms over warm. `scripts/startup-bench.py` still ships the wrong Windows
   paths (and its `--purge-glob` override, referenced above, doesn't actually
   exist) — fix `webview_data_dirs()` before trusting its Windows cold numbers.

- **`.exe` naming.** The harness appends `.exe` to its throwaway copies on
  Windows automatically — that part works.
- **WPA capture not done.** A full WPR/WPA symbolicated capture (the Windows
  analog of macOS `sample`) needs elevation and was not run. It's unnecessary to
  place the cost: the phase timeline already pins the warm quarter-second to the
  `run()`→`setup()` gap, i.e. WebView2 + Win32 window creation. Run WPA only if
  someone wants that gap broken down further.
