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
