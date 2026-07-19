# Cutting a release

Prebuilt Windows and macOS apps are published to the repo's
[Releases](https://github.com/RobinMorisset/OpenShogiPairings/releases) page by a
GitHub Actions workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml)). You don't
build or upload anything by hand — you push a version tag and the workflow does
the rest.

## Steps

1. **Bump the version** so the tag matches what the app reports. Update the
   `version` field in all four:
   - [`Cargo.toml`](../Cargo.toml) (`[workspace.package]` — feeds
     `osp_core::VERSION`, i.e. the `/api/health` payload and the client's
     "server upgraded" check)
   - [`frontend/src-tauri/Cargo.toml`](../frontend/src-tauri/Cargo.toml) (the
     desktop crate is excluded from the workspace, so it carries its own
     `[package]` version)
   - [`frontend/src-tauri/tauri.conf.json`](../frontend/src-tauri/tauri.conf.json)
   - [`frontend/package.json`](../frontend/package.json)

   Commit that on `main`. All four must equal the tag you're about to push —
   CI's `check-version` job verifies this and fails the release before building
   if any is out of sync.

   > **Not** part of the app version bump:
   > [`crates/matching`](../crates/matching/Cargo.toml) (`integer-blossom`) is
   > published to crates.io independently and carries its own `[package]`
   > version, deliberately decoupled from the workspace. Leave it alone when
   > cutting an app release — it is bumped only when the matching crate itself
   > changes and is released on its own cadence (see below). `check-version`
   > does not gate it, and must not: its version is meant to diverge from the
   > app's.

   Bump the root `version` in
   [`frontend/package-lock.json`](../frontend/package-lock.json) to match too —
   both the top-level field and the one under `packages[""]`, so the lockfile
   mirrors `package.json`. Nothing re-syncs it automatically (unlike the two
   `Cargo.lock`s, which the pre-commit hook re-resolves), so it drifts otherwise;
   `npm install` in `frontend/` updates it, or edit the two lines by hand. It is
   not part of the `check-version` gate.

2. **Tag and push:**

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

   Use `vX.Y.Z` — the leading `v` is what the workflow's tag filter (`v*`)
   matches.

3. **Watch the build** on the repo's **Actions** tab. Two jobs run in parallel (a
   `macos-latest` runner and a `windows-latest` runner), each roughly 10–20 min.
   The macOS job builds a universal binary (Apple Silicon + Intel).

4. **Publish the draft.** When both jobs finish, a **draft** GitHub Release
   appears on the Releases page with the installers attached (`.dmg`/`.app` for
   macOS, `.exe`/`.msi` for Windows). Review the notes and assets, then click
   **Publish release** to make it public.

## Testing the workflow without cutting a release

Use the **Run workflow** button on the Actions tab — the workflow allows manual
`workflow_dispatch`. It builds on both platforms without needing a tag, so you
can confirm the pipeline is green before committing to a real version.

## Releasing the `integer-blossom` crate

[`crates/matching`](../crates/matching/Cargo.toml) is published to crates.io on
its own schedule, independent of the desktop app above. Its version tracks *its*
public API, not the app tag, so it moves only when the matching crate changes.

To cut a crate release: bump `version` in
[`crates/matching/Cargo.toml`](../crates/matching/Cargo.toml) (follow semver for
the crate's own API — patch for fixes, minor for additions, major for breaking
changes), add a matching entry to
[`crates/matching/CHANGELOG.md`](../crates/matching/CHANGELOG.md) (crates.io has
no changelog of its own; this file ships in the package and renders on docs.rs),
commit on `main`, then from `crates/matching/` run `cargo publish`
(`cargo publish --dry-run` first to sanity-check the package). Publish only after
the changelog entry is committed — like the crate itself, a published version is
immutable. This is a separate act from tagging an app release; neither triggers
the other.

## Notes

- **Unsigned builds.** The apps are not code-signed, so users get an "unknown
  publisher" (Windows) or "unidentified developer / damaged" (macOS) warning on
  first launch. This is documented for users in the README's
  [Download](../README.md#download) section. Removing it means buying an Apple
  Developer ID (~$99/yr) and a Windows code-signing certificate, then adding the
  signing secrets to the workflow — see the
  [tauri-action docs](https://github.com/tauri-apps/tauri-action).
- **Draft by default.** The workflow sets `releaseDraft: true`, so a bad build
  never goes public on its own. Flip it to `false` in the workflow once you trust
  the pipeline and want a tag push to publish automatically.
- **Local one-off build.** To produce a single executable locally without the
  release machinery, see [Packaging](../README.md#packaging-windows) in the
  README.
