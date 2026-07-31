# Publishing guide

The GitHub release workflow builds all six native targets from a `vX.Y.Z` tag.
It publishes the raw binaries, ready-to-publish platform package tarballs,
target-specific CycloneDX SBOMs, SHA-256 checksums, Sigstore bundles, and
GitHub build provenance. It does not provide Windows Authenticode signing or
Apple notarization.

## 1. Create the signed GitHub release

The tag must exactly match the version in `package.json` and `Cargo.toml`.
Run the relevant commands from a clean checkout of `main`.

### macOS and Linux

```bash
VERSION=$(node -p "require('./package.json').version")
git status --short
git tag "v$VERSION"
git push origin "v$VERSION"
RUN_ID=$(gh run list --repo Aminoragit/my-token-scrooge --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$RUN_ID" --repo Aminoragit/my-token-scrooge --exit-status
gh release view "v$VERSION" --repo Aminoragit/my-token-scrooge
```

### Windows PowerShell

```powershell
$Version = node -p "require('./package.json').version"
git status --short
git tag "v$Version"
git push origin "v$Version"
$RunId = gh run list --repo Aminoragit/my-token-scrooge --workflow release.yml --limit 1 --json databaseId --jq ".[0].databaseId"
gh run watch $RunId --repo Aminoragit/my-token-scrooge --exit-status
gh release view "v$Version" --repo Aminoragit/my-token-scrooge
```

Verify a downloaded file with Cosign 3:

```bash
cosign verify-blob \
  --bundle mts-linux-x64.sigstore.json \
  --certificate-identity "https://github.com/Aminoragit/my-token-scrooge/.github/workflows/release.yml@refs/tags/v0.1.0" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  mts-linux-x64
```

GitHub provenance can also be verified with:

```bash
gh attestation verify my-token-scrooge-linux-x64-0.1.0.tgz \
  --repo Aminoragit/my-token-scrooge
```

Each `mts-<platform>.cdx.json` SBOM has its own Sigstore bundle and is verified
with the same `cosign verify-blob` command as a binary.

## 2. Publish the npm packages manually

You need an npm account that controls the `@my-token-scrooge` scope and can
publish the unscoped `my-token-scrooge` package. Enable two-factor authentication
before publishing. A package version cannot be reused after publication.

Publish the six native packages first. Publish the root wrapper last so users
never receive a wrapper whose optional native packages are missing.

### macOS and Linux

```bash
VERSION=$(node -p "require('./package.json').version")
DIST="publish-$VERSION"
mkdir "$DIST"
gh release download "v$VERSION" \
  --repo Aminoragit/my-token-scrooge \
  --pattern 'my-token-scrooge-*.tgz' \
  --dir "$DIST"

npm whoami
for PLATFORM in linux-x64 linux-arm64 darwin-x64 darwin-arm64 win32-x64 win32-arm64; do
  npm publish "$DIST/my-token-scrooge-$PLATFORM-$VERSION.tgz" --access public
done

npm pack --pack-destination "$DIST"
npm publish "$DIST/my-token-scrooge-$VERSION.tgz" --access public
```

### Windows PowerShell

```powershell
$Version = node -p "require('./package.json').version"
$Dist = ".\publish-$Version"
New-Item $Dist -ItemType Directory | Out-Null
gh release download "v$Version" `
  --repo Aminoragit/my-token-scrooge `
  --pattern "my-token-scrooge-*.tgz" `
  --dir $Dist

npm whoami
$Platforms = "linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64", "win32-arm64"
foreach ($Platform in $Platforms) {
  npm publish "$Dist\my-token-scrooge-$Platform-$Version.tgz" --access public
  if ($LASTEXITCODE -ne 0) { throw "Publishing $Platform failed; do not publish the root package." }
}

npm pack --pack-destination $Dist
npm publish "$Dist\my-token-scrooge-$Version.tgz" --access public
```

Confirm the published versions and test a clean install:

```bash
npm view my-token-scrooge version
npm view @my-token-scrooge/linux-x64 version
npx --yes my-token-scrooge@0.1.0 --version
```

Local npm publication cannot create npm provenance. If npm provenance is later
required, configure npm trusted publishing and publish from GitHub Actions.
