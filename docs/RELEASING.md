# Releasing lokal

How a maintainer cuts a release. Two GitHub Actions workflows do the work:

- **`.github/workflows/release.yml`** runs when a `vX.Y.Z` tag is pushed. It runs every CI check (`ci.yml`), then:
  - builds and smoke-tests a tarball for each macOS architecture (`scripts/package.sh`);
  - builds a tarball, a `.deb` and an `.rpm` for each Linux architecture on that architecture's runner, and
    smoke-tests them in containers of the target distributions (`scripts/smoke-linux.sh`);
  - installs the Homebrew formula, rendered from the new tarballs, on macOS and both Linux runners;
  - creates a **draft** release with every artifact, their checksums, and the version's CHANGELOG section as the
    notes (`scripts/release-notes.sh`).
- **`.github/workflows/homebrew.yml`** runs when you publish the draft. It writes `Formula/lokal.rb` in
  [mehmetkose/homebrew-tap](https://github.com/mehmetkose/homebrew-tap) from `packaging/homebrew/lokal.rb.in`
  (`scripts/homebrew-formula.sh`). Then it installs lokal from the tap on fresh macOS and Linux runners and runs
  `brew test` and `brew audit --strict --online`.

Only maintainers can see a draft, so nothing reaches users until you publish it.

## Cutting a release

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml`, then run `cargo build` so `Cargo.lock`
   follows.
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z]` and add a new, empty `## [Unreleased]` above it.
   Update the link references at the bottom. Bump the version in the install snippets and examples too:
   `README.md` and `website/site/docs/installation.md` (`v=X.Y.Z`, and the `lokal --version` example),
   `website/theme/data/site.ts` (`version`), and add the section to `website/site/docs/changelog.md`.
3. Run the pre-release cycle from [TESTING.md](TESTING.md#clean-room): `lokal uninstall`, `scripts/clean-check.sh`,
   `lokal setup`, then `scripts/live-test.sh --full`. Save the report to `docs/live-test-reports/`.
4. Commit, push to `main`, and wait for CI to pass.
5. Tag the commit and push the tag:

   ```console
   $ git tag vX.Y.Z
   $ git push origin vX.Y.Z
   ```

   The workflow fails if the tag doesn't match the version in `Cargo.toml`, or if the CHANGELOG has no section
   for it.
6. Open the draft on the [releases page](https://github.com/mehmetkose/lokal/releases), check the assets and the
   notes, and publish it. A release has 17 assets: four tarballs, two `.deb`, two `.rpm`, a `.sha256` for each of
   those eight, and `SHA256SUMS`.
7. Watch the Homebrew workflow (`gh run watch`). Once its `verify` jobs pass on all three runners,
   `brew upgrade lokal` installs the new version.

A tag with a suffix, such as `v0.2.0-rc.1`, makes a pre-release. The Homebrew workflow skips pre-releases.

## Dry run

To test the build without a tag, run the release workflow by hand:

```console
$ gh workflow run release.yml
$ gh run watch
```

It runs every job but the last: it builds and smoke-tests every artifact, installs the formula from them, and
uploads them as workflow artifacts, but it doesn't create a release. Add `--ref <branch>` to try a branch.

Locally, `scripts/package.sh aarch64-apple-darwin` builds the macOS tarball in `dist/`. The Linux targets build
only on Linux of the same architecture, with `dpkg-deb` and `rpmbuild` (`apt-get install rpm`). On a Mac, an
`ubuntu:24.04` container works (podman or Docker on Apple Silicon gives arm64). Then
`DOCKER=podman scripts/smoke-linux.sh aarch64-unknown-linux-gnu` runs the smoke test from the Mac.

## Rerunning the tap update

If the Homebrew workflow fails after a release is published, fix the cause and run it again for that version:

```console
$ gh workflow run homebrew.yml -f version=X.Y.Z
```

## One-time setup

Create the tap repository, and give the release workflow a deploy key that can write to it:

```console
$ gh repo create mehmetkose/homebrew-tap --public --add-readme --description "Homebrew formulae for lokal"
$ ssh-keygen -t ed25519 -N '' -C lokal-release -f tap_key
$ gh repo deploy-key add tap_key.pub -R mehmetkose/homebrew-tap --allow-write -t lokal-release
$ gh secret set HOMEBREW_TAP_DEPLOY_KEY -R mehmetkose/lokal < tap_key
$ rm tap_key tap_key.pub
```

The deploy key can write only to the tap repository, and unlike a personal access token, it doesn't expire.
`--add-readme` gives the tap a first commit, which the workflow needs before it can check the tap out.

## Signing

The macOS binaries are ad-hoc signed, but not signed with an Apple Developer ID or notarized
([PACKAGING.md](PACKAGING.md#prebuilt-binaries)). Adding notarization later needs a Developer ID certificate and
an App Store Connect API key as repository secrets, plus `codesign` and `notarytool` steps in
`scripts/package.sh`. Nothing else in the pipeline would change. The Linux tarballs and packages aren't signed;
the checksums are their only check.
