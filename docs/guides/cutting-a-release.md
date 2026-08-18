# Cutting a release

Prebuilt Windows and macOS apps are published to the repo's
[Releases](https://github.com/RobinMorisset/OpenShogiPairings/releases) page by a
GitHub Actions workflow
([`.github/workflows/release.yml`](../../.github/workflows/release.yml)). You don't
build or upload anything by hand — you push a version tag and the workflow does
the rest.

## Steps

1. **Close the changelog section.** In [`CHANGELOG.md`](../../CHANGELOG.md), rename
   the `## [Unreleased]` heading to `## [X.Y.Z] - YYYY-MM-DD` with the version
   you're about to tag and the release date, and reread the section: it is where
   the "NOT backwards compatible with any earlier version" banner lives, and that
   line is the only warning a user gets that their save files won't load. Do it
   first, before the version bump, so the version you write here is the one you
   then propagate.

   CI's `check-version` job refuses to build a release whose `CHANGELOG.md` still
   has an `## [Unreleased]` heading, or whose topmost `## [version]` section
   isn't the tag being pushed. It does not read the *contents* of that section —
   writing the entries is still on you.

   Don't open a fresh empty `## [Unreleased]` section in the same commit; add it
   back when the first post-release change lands.

2. **Bump the version** so the tag matches what the app reports. Update the
   `version` field in all four:
   - [`Cargo.toml`](../../Cargo.toml) (`[workspace.package]` — feeds
     `osp_core::VERSION`, i.e. the `/api/health` payload and the client's
     "server upgraded" check)
   - [`frontend/src-tauri/Cargo.toml`](../../frontend/src-tauri/Cargo.toml) (the
     desktop crate is excluded from the workspace, so it carries its own
     `[package]` version)
   - [`frontend/src-tauri/tauri.conf.json`](../../frontend/src-tauri/tauri.conf.json)
   - [`frontend/package.json`](../../frontend/package.json)

   Commit that on `main`, together with the changelog edit from step 1. All four
   must equal the tag you're about to push — CI's `check-version` job verifies
   this and fails the release before building if any is out of sync.

   > **Not** part of the app version bump:
   > [`crates/matching`](../../crates/matching/Cargo.toml) (`integer-blossom`) is
   > published to crates.io independently and carries its own `[package]`
   > version, deliberately decoupled from the workspace. Leave it alone when
   > cutting an app release — it is bumped only when the matching crate itself
   > changes and is released on its own cadence (see below). `check-version`
   > does not gate it, and must not: its version is meant to diverge from the
   > app's. The flip side of that: nothing will remind you, so if
   > `crates/matching` changed this cycle, note it now and cut a crate release
   > separately (see [below](#releasing-the-integer-blossom-crate)).

   Bump the root `version` in
   [`frontend/package-lock.json`](../../frontend/package-lock.json) to match too —
   both the top-level field and the one under `packages[""]`, so the lockfile
   mirrors `package.json`. Nothing re-syncs it automatically (unlike the two
   `Cargo.lock`s, which the pre-commit hook re-resolves), so it drifts otherwise;
   `npm install` in `frontend/` updates it, or edit the two lines by hand. It is
   not part of the `check-version` gate.

3. **Tag and push:**

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

   Use `vX.Y.Z` — the leading `v` is what the workflow's tag filter (`v*`)
   matches.

   The tag is also what makes the build stop advertising a commit. Every build
   bakes in its `git describe` (`crates/core/build.rs`) and the app's status
   line shows it — `Server openshogipairings-server v1.3.0
   (v1.3.0-14-ga031b12)`, fourteen commits past that release — because the
   version number alone can't tell two builds between releases apart. A build
   whose Rust sources had uncommitted changes appends `-dirty`, since no commit
   describes it. When they are clean and HEAD carries a tag naming exactly this
   version (`1.3.0` or `v1.3.0`), the description is dropped: the version *is*
   the identity of the build. So the released installers show the bare `v1.3.0`,
   and anything else says how far it strayed from one — and whether even that
   was the whole truth.

   Only `v*` tags count towards this, so a tag that isn't an app release — a
   scratch marker, or an [`integer-blossom`](#releasing-the-integer-blossom-crate)
   crates.io release, should you ever start tagging those — can't become the
   name this app answers to. Keep such tags off the `v*` spelling.

   Which means a release whose *server* was built from uncommitted changes is
   visibly labelled as one. If you ever see `-dirty` on something you meant to
   publish, the installers don't match the tag, and the fix is to commit (or
   discard) the difference and rebuild rather than to ship it.

   Read `-dirty` narrowly, though: it is measured over `RUST_SOURCES` in
   `crates/core/build.rs` — the crates, the Cargo manifests, the toolchain file
   — and not over the rest of the tree. That is what the string is *about*: it
   is reported by the server, over an endpoint that answers for the server, and
   an uncommitted `.svelte` file cannot change a byte of that binary. The cost
   of the wider reading was a rebuild of `osp-core` and everything downstream on
   the next `cargo` command after editing any file at all, doc or otherwise. The
   gap it leaves is a locally built desktop app, which bundles the web client:
   its server can be clean while the UI around it was not committed. CI builds
   releases from a fresh checkout, so a released installer cannot be in that
   state.

4. **Watch the build** on the repo's **Actions** tab. The workflow runs four jobs
   in three stages:

   - `check-version` — the version and changelog gates from steps 1–2, seconds,
     no build runners spun up.
   - `create-release` — creates the **draft** GitHub Release up front, so it is
     already on the Releases page (empty) while the builds are still running.
     Both build runners then upload into that one release; they used to each
     create their own, which is what produced duplicate per-platform releases.
   - `build` — two matrix jobs in parallel, a `macos-latest` runner and a
     `windows-latest` runner, each roughly 10–20 min. The macOS job builds a
     universal binary (Apple Silicon + Intel).

5. **Publish the draft.** When both build jobs finish, the draft Release has the
   installers attached (`.dmg`/`.app` for macOS, `.exe`/`.msi` for Windows).
   Review the notes and assets, then click **Publish release** to make it public.
   The draft's notes are a one-line placeholder written by `create-release`; the
   real notes are the changelog section from step 1, so paste or summarise it
   here before publishing.

## Testing the workflow without cutting a release

Use the **Run workflow** button on the Actions tab — the workflow allows manual
`workflow_dispatch`. It builds on both platforms without needing a tag, so you
can confirm the pipeline is green before committing to a real version. With no
tag, `check-version` and `create-release` are both skipped, and `build` runs with
an empty `releaseId`, so `tauri-action` builds the installers without publishing
them anywhere.

## Releasing the `integer-blossom` crate

[`crates/matching`](../../crates/matching/Cargo.toml) is published to crates.io on
its own schedule, independent of the desktop app above. Its version tracks *its*
public API, not the app tag, so it moves only when the matching crate changes.

To cut a crate release: bump `version` in
[`crates/matching/Cargo.toml`](../../crates/matching/Cargo.toml) (follow semver for
the crate's own API — patch for fixes, minor for additions, major for breaking
changes), add a matching entry to
[`crates/matching/CHANGELOG.md`](../../crates/matching/CHANGELOG.md) (crates.io has
no changelog of its own; this file ships in the package and renders on docs.rs),
commit on `main`, then from `crates/matching/` run `cargo publish`
(`cargo publish --dry-run` first to sanity-check the package). Publish only after
the changelog entry is committed — like the crate itself, a published version is
immutable. This is a separate act from tagging an app release; neither triggers
the other.

## Notes

- **Unsigned builds.** The apps carry no trusted signature, so users get an
  "unknown publisher" (Windows) or "unidentified developer" (macOS) warning on
  first launch. This is documented for users in the README's
  [Download](../../README.md#download) section. Removing it means buying an Apple
  Developer ID (~$99/yr) and a Windows code-signing certificate, then adding the
  signing secrets to the workflow — see the
  [tauri-action docs](https://github.com/tauri-apps/tauri-action).
- **The macOS bundle is ad-hoc signed** — that's `bundle.macOS.signingIdentity`
  set to `"-"` in [`tauri.conf.json`](../../frontend/src-tauri/tauri.conf.json),
  which JSON gives no way to explain in place, hence this note. An ad-hoc
  signature carries no identity and buys no trust, so Gatekeeper still stops the
  app; what it avoids is the *worse* dialog. Apple Silicon refuses to run a
  binary with no signature at all, and a downloaded bundle that fails validation
  outright is reported as "damaged" and offered to the Trash rather than as
  merely unverified. Don't drop the setting to "simplify" the config.
- **Draft by default.** `create-release` passes `--draft` to `gh release create`,
  so a bad build never goes public on its own — publishing is always the manual
  click in step 5. Dropping that flag would make a tag push publish
  automatically, notes and all; don't, while the release notes still have to be
  filled in by hand.
- **Local one-off build.** To produce a single executable locally without the
  release machinery, see [Packaging](../../README.md#packaging-windows) in the
  README.
