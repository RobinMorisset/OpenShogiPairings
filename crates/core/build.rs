//! Bake this build's `git describe` into `osp_core::GIT_DESCRIPTION`.
//!
//! The version alone (`1.3.0`) says nothing about *which* build of 1.3.0 is
//! running: every commit between two releases reports the same number. So the
//! health payload also carries what `git describe` makes of the build —
//! `v1.3.0-14-ga031b12`, i.e. fourteen commits past that release, suffixed
//! `-dirty` when the sources it was built from had uncommitted changes and no
//! commit describes it at all. The one case with nothing to add is a clean tree
//! whose HEAD carries a tag naming this very version: that build *is* the
//! release, and the version already identifies it.
//!
//! "The sources it was built from" is meant narrowly, and [`RUST_SOURCES`] is
//! the list. This string is only ever reported by a Rust binary, about itself —
//! the health endpoint answers for the server, and nothing here claims to
//! describe the web client it is serving. Editing a `.svelte` file or a doc
//! leaves the server byte-identical to the one HEAD would produce, so calling it
//! dirty would be noise; it would also cost a recompile of this crate and
//! everything downstream on the next `cargo` command, for a string that could
//! not have changed.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Everything a Rust build of this repository reads. Dirtiness is measured over
/// these paths, and they are the ones cargo is asked to watch.
///
/// `frontend/src-tauri` earns its place: it is the desktop app's own crate, and
/// it links the server in. The rest of `frontend/` does not — those files ship
/// inside the desktop bundle, but they cannot change a byte of the binary that
/// answers `/api/health`.
const RUST_SOURCES: &[&str] = &[
    "crates",
    "frontend/src-tauri",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo always sets CARGO_PKG_VERSION");
    println!(
        "cargo:rustc-env=OSP_GIT_DESCRIPTION={}",
        git_description(&version)
    );
}

/// How git describes this build, or an empty string when there is nothing worth
/// saying (HEAD is the release tag, or this isn't a git checkout at all).
fn git_description(version: &str) -> String {
    // Not a git checkout: a source tarball or a vendored copy. Nothing to read,
    // and refusing to build would be wrong — but say so, because the far more
    // likely cause is a build environment that dropped `.git` by accident and
    // would otherwise silently start reporting bare version numbers.
    let Some(git) = GitDirs::locate() else {
        println!(
            "cargo:warning=osp-core: not a git checkout (or `git` is unavailable), so the \
             reported server version will carry no build description."
        );
        return String::new();
    };

    git.watch_everything_the_answer_depends_on();

    let described = git.run(&[
        "describe",
        // Anchor on app release tags only, so that any other tag in the history
        // — a scratch marker, or a crates.io release of `crates/matching`, which
        // ships on its own cadence — can't become the name this app answers to.
        "--tags", "--match", "v*",
        // No reachable tag — a shallow or tagless clone, which is what CI hands
        // a manually dispatched (untagged) build. Fall back to the bare hash
        // instead of failing outright, as plain `describe` would.
        "--always",
        // Deliberately not `--dirty`: that asks about the whole tree, and the
        // answer here is about [`RUST_SOURCES`] alone. Appended below instead.
    ]);

    // Exactly the release tag with nothing appended — no distance: the version
    // *is* the identity of this build, so a clean one has nothing to add. Both
    // spellings, since the repo tags `v1.3.0` while the crate says `1.3.0`.
    let is_this_release = described == version || described.strip_prefix('v') == Some(version);

    // Uncommitted Rust sources mean no commit describes this build, so say so
    // even when HEAD is tagged — a modified release is not that release. Staged
    // and unstaged both count (both differ from HEAD); untracked files don't, or
    // every stray scratch file in the tree would brand the build dirty.
    let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
    status.extend_from_slice(RUST_SOURCES);
    if !git.run(&status).is_empty() {
        return format!("{described}-dirty");
    }

    if is_this_release {
        return String::new();
    }

    described
}

/// The git directories for this checkout: `this` is per-worktree (where HEAD
/// and the index live), `common` is shared by every worktree (where refs live).
/// They are the same path outside a linked worktree.
struct GitDirs {
    this: PathBuf,
    common: PathBuf,
    top_level: PathBuf,
}

impl GitDirs {
    /// Locate the git directories, or `None` if this isn't a checkout we can read.
    fn locate() -> Option<Self> {
        let output = Command::new("git")
            .args([
                "rev-parse",
                "--absolute-git-dir",
                "--git-common-dir",
                "--show-toplevel",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let mut lines = text.lines();
        let this = PathBuf::from(lines.next()?);
        // `--git-common-dir` answers relative to the cwd when it resolves to a
        // plain `.git`, so don't trust it to be absolute.
        let common = lines.next()?;
        let common = if Path::new(common).is_absolute() {
            PathBuf::from(common)
        } else {
            std::env::current_dir().ok()?.join(common)
        };
        let top_level = PathBuf::from(lines.next()?);
        Some(Self {
            this,
            common,
            top_level,
        })
    }

    /// Tell cargo every path that could change the answer, so a cached build
    /// script result can't outlive the state it recorded.
    ///
    /// The commit and the tag it is measured from move with `HEAD` and the ref
    /// files. Dirtiness moves with the tracked files under [`RUST_SOURCES`] —
    /// which is why they are named one by one: naming instead the directories
    /// they live in would mean scanning `target/`, and naming nothing at all
    /// would let a build script that ran while they were clean keep claiming so
    /// long after it stopped being true. `index` covers `git add` of a file that
    /// wasn't tracked when this list was drawn up.
    ///
    /// This list and the dirtiness check must span the same paths. Watch less
    /// and the answer goes stale; watch more — every tracked file in the repo,
    /// as this once did — and editing a `.svelte` file or a doc rebuilds this
    /// crate and everything downstream to recompute a string that cannot have
    /// changed.
    ///
    /// Cargo re-runs the script when a listed path is *missing* too, so deleting
    /// a tracked file is caught by the same list.
    fn watch_everything_the_answer_depends_on(&self) {
        watch(&self.this.join("HEAD"));
        watch(&self.this.join("index"));
        watch(&self.common.join("refs"));
        watch(&self.common.join("packed-refs"));

        // `-z` because git otherwise quotes paths that aren't plain ASCII, and a
        // quoted path names no file on disk — cargo would then re-run the script
        // on every single build.
        for source in RUST_SOURCES {
            let files = self.run_raw(&["ls-files", "-z", "--", source]);
            let mut watched = 0;
            for file in files.split('\0').filter(|f| !f.is_empty()) {
                watch(&self.top_level.join(file));
                watched += 1;
            }
            // One entry at a time, so that a path which has been renamed or
            // deleted since this list was written says so, instead of silently
            // contributing nothing — leaving its files unwatched and its changes
            // invisible to the dirtiness check that spans the very same list.
            assert!(
                watched > 0,
                "osp-core build script: RUST_SOURCES names {source:?}, which matches no tracked \
                 file. It has moved or gone; update the list."
            );
        }
    }

    /// Run a git command that we already know must work, since [`GitDirs::locate`]
    /// proved git is present and this is a checkout. A failure here is a broken
    /// repository or a broken git, and guessing past it would bake in a wrong
    /// commit.
    ///
    /// The output is trimmed; use [`GitDirs::run_raw`] where trailing bytes
    /// carry meaning.
    fn run(&self, args: &[&str]) -> String {
        self.run_raw(args).trim().to_string()
    }

    /// As [`GitDirs::run`], but returning the output untrimmed — `-z` records are
    /// separated, not terminated the way lines are.
    ///
    /// Every command runs at the repository top level. That is not tidiness: git
    /// resolves a pathspec relative to the *current directory*, and this script
    /// runs in `crates/core`, so `-- crates` asked from here would quietly mean
    /// `crates/core/crates`, match nothing, and report a modified tree as clean.
    fn run_raw(&self, args: &[&str]) -> String {
        let top_level = self.top_level.to_str().expect("cargo paths are UTF-8");
        let mut full = vec!["-C", top_level];
        full.extend_from_slice(args);
        let output = Command::new("git")
            .args(&full)
            .output()
            .unwrap_or_else(|err| {
                panic!(
                    "osp-core build script: `git {}` failed: {err}",
                    full.join(" ")
                )
            });
        assert!(
            output.status.success(),
            "osp-core build script: `git {}` exited with {}: {}",
            full.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8(output.stdout).expect("git writes UTF-8 ref names, paths and hashes")
    }
}

/// Ask cargo to re-run this script when `path` changes. Emitted only for paths
/// that exist: naming a missing one makes cargo re-run the script every build.
fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
