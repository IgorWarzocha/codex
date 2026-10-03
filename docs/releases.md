# Releasing Codex Lean

Codex Lean uses its own GitHub Actions workflow, `lean-release.yml`, and publishes
archives to this fork's GitHub Releases. Binaries are not committed to Git. The
upstream release workflow depends on OpenAI infrastructure and is not the fork's
release path.

## Versioning

Versions retain the upstream base and add a fork revision:

```text
0.160.0-lean.1
```

The version in `codex-rs/Cargo.toml` is authoritative. Keep workspace package
versions in `codex-rs/Cargo.lock` synchronized without upgrading dependencies.
Release tags use `lean-v<VERSION>`, for example `lean-v0.160.0-lean.1`.
Never move a published tag or replace its assets. Use a new fork revision for a
changed release.

## Build and publish

Run the **Codex Lean release** workflow from the repository's Actions page, selecting
the `lean` branch. Leave publishing disabled to produce CI artifacts only.
Enable publishing to create a GitHub prerelease after every platform has built,
packaged, and passed its smoke checks. The release tag identifies the exact
commit that was built, not whichever commit happens to be latest when the jobs
finish.

With GitHub CLI:

```sh
# Build and validate without publishing.
gh workflow run lean-release.yml --repo IgorWarzocha/codex-lean --ref lean

# Build, validate, and publish a prerelease.
gh workflow run lean-release.yml --repo IgorWarzocha/codex-lean --ref lean -f publish=true
```

To diagnose a Windows native voice failure without rebuilding the other platforms:

```sh
gh workflow run lean-release.yml --repo IgorWarzocha/codex-lean --ref lean -f scope=windows-voice
```

This diagnostic scope produces only the intermediate Windows voice artifact. It
does not build CLI packages and cannot publish a release. After it passes, use the
default `all` scope to build and validate all complete packages from one commit.
Rerunning an old job does not pick up a source fix; dispatch on the updated branch.

A matching `lean-v<VERSION>` tag can also start a release. Tag and workspace
versions must agree. A failed platform prevents publication; inspect and fix the
failure rather than dropping that platform from the release being validated.

The workflow uses public GitHub-hosted runners and pinned build actions. Build
jobs have read-only repository access. Only the publication job can write release
assets. It does not publish to OpenAI's package registries or use OpenAI signing
credentials.

## Package contents and boundaries

The existing package builder owns the layout, helper selection, and verified
downloads. Releases contain optimized, stripped binaries, a package manifest,
the Code Mode host, ripgrep, platform-specific sandbox helpers, and the matching
native voice helper and audio runtime. Linux packages embed the digest of the
finalized bubblewrap binary before compiling the CLI.

Archives are named `codex-lean-<VERSION>-<TARGET>.tar.gz`, or `.zip` on Windows.
`SHA256SUMS` records their digests. Checksums detect changed downloads; they do not
replace review of the source or trust in the publishing account.

Initial targets are Linux x64 and ARM64, Apple Silicon macOS, and Windows x64.
Smoke checks run the packaged binaries on each native runner. They do not prove
support for older operating systems, live provider authentication, or every
interactive feature.

macOS and Windows packages are not developer-signed or notarized. The voice helper
and privately bundled audio runtime must pass their packaging and runtime checks
before publication. A platform build failure blocks the release; omitting voice
is not a fallback. CI does not establish a successful live microphone call.

Users update by downloading a new fork package. The fork does not run upstream's
automatic installer, which would replace Codex Lean with stock Codex.
