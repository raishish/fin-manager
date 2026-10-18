# Releases

How fin-manager is built and published, now (T5.11: unsigned builds) and next (T5.7:
signing, notarisation, the model check and update checks).

## What the release workflow does

`.github/workflows/release.yml` builds:

| Platform | Files |
|---|---|
| macOS, Apple Silicon | `fin-manager_<version>_aarch64.dmg` and `fin-manager_<version>_aarch64.app.zip` |
| Linux, x86-64 | `fin-manager_<version>_amd64.AppImage` |
| Both | `SHA256SUMS`, and `fin-manager_<version>.spdx.json` (SBOM) |

Jobs:

1. **Version.** Reads the version from `app/src-tauri/tauri.conf.json` and checks that
   the workspace `Cargo.toml` says the same. For a tag, the tag must be `v<version>`.
2. **macOS.** `tauri build --bundles app,dmg` on `macos-14`. The app is ad-hoc signed
   (`bundle.macOS.signingIdentity: "-"`), so it runs on Apple Silicon but isn't
   trusted by Gatekeeper. The job checks that the binary is arm64 and that the
   signature verifies.
3. **Linux.** `tauri build --bundles appimage` on `ubuntu-24.04`. The job then
   **smoke-tests** the AppImage: it starts it under Xvfb with an empty home folder and
   requires it to still be running after 30 seconds without a panic or a crash.
4. **SBOM.** [syft](https://github.com/anchore/syft) reads `Cargo.lock` and
   `app/package-lock.json` (not the test-data workspaces) and writes SPDX JSON.
5. **Checksums.** Collects every file and writes `SHA256SUMS`.
6. **Draft release** (tag pushes only). Creates a **draft** GitHub release with all of
   the files and notes saying the build is unsigned. Nothing is published until a
   person reviews the draft and publishes it.

A **dry run** does steps 1 to 5 and keeps the files as workflow artifacts for 14 days,
with no release. Dry runs happen:

- on every pull request that changes the release configuration
  (`.github/workflows/release.yml`, `app/src-tauri/tauri.conf.json`,
  `app/src-tauri/Cargo.toml`). The pull request's run is the evidence that a release
  would still build. Fork pull requests need a maintainer's approval to run, and get a
  read-only token and no secrets.
- by hand ("Run workflow", `workflow_dispatch`).

### Supply chain

- Every action is pinned by full commit SHA (SPEC decision 35). Downloaded tools are
  pinned by version and SHA-256:
  - syft 1.54.1;
  - Tauri's AppImage tools, which Tauri would otherwise download at build time, one
    of them from a moving `continuous` release:

    | Tool | Source | SHA-256 |
    |---|---|---|
    | `AppRun-x86_64` | `tauri-apps/binary-releases`, `apprun-old` | GitHub's published digest |
    | `linuxdeploy` `07333c6` | `tauri-apps/binary-releases`, `linuxdeploy-07333c6` | GitHub's published digest |
    | `linuxdeploy-plugin-appimage` `1-alpha-20250213-1` | `linuxdeploy/linuxdeploy-plugin-appimage` | Computed from one download on 2026-10-17; that release publishes no digest |

    The file names and URLs are those of `@tauri-apps/cli` 2.12.3. Re-check them
    when the CLI version changes (`crates/tauri-bundler/src/bundle/linux/appimage/linuxdeploy.rs`
    at the CLI's tag).

    The plugin's SHA-256 is trust on first use: computed from a single download, with
    nothing published to compare it with. **Re-check it by 2027-04-17** (or at T5.7,
    whichever comes first). Download it again from a different network and confirm
    the hash, or switch to a release that publishes a digest.
- `cargo build --locked`, `npm ci` (lockfile integrity) and `rustup` (the pinned
  toolchain in `rust-toolchain.toml`) check what they download.
- No build cache is restored: a release is built from the tagged source alone.
- The workflow uses **no secrets**. Its token is read-only, except in the draft-release
  job (`contents: write`), which only runs for a tag push. Checkouts don't keep the
  token.

### Known gaps

- Ubuntu's apt packages (WebKitGTK and friends) can't be pinned by hash, as in CI.
- Node comes from `actions/setup-node`, as in CI.
- The AppImage is built on Ubuntu 24.04, so it needs a distribution with glibc 2.39 or
  newer. Building on an older base for wider reach is a T5.7 decision.
- The SBOM covers the whole Rust workspace and the app's npm lockfile, development
  dependencies included, so it lists more than ships.
- The macOS build is Apple Silicon only (SPEC decision 11).
- It needs macOS 11 or later (`bundle.macOS.minimumSystemVersion`): llama.cpp, built into the app since T5.5, uses `std::filesystem`, which needs macOS 10.15, and Apple Silicon Macs run 11 or later anyway.

## Cutting a release

1. Set the same version in the workspace `Cargo.toml` (`[workspace.package] version`)
   and in `app/src-tauri/tauri.conf.json` (`version`), and land that on `main`.
2. Check that CI on that commit is green.
3. Tag the commit `v<version>` (a signed tag, SPEC decision 36) and push the tag.
4. When the workflow finishes, review the draft release:
   - the files are all there;
   - `SHA256SUMS` matches what you download (`sha256sum --check SHA256SUMS`);
   - the app opens on a Mac and on Linux.
5. Publish the draft.

To check the pipeline without releasing, open a pull request that touches the release
configuration (its run is a dry run), or run the workflow by hand on `main`.

## Next: T5.7

T5.7 starts once the user has an Apple Developer ID (before Stage 1). Each item below
is a plan, and items marked **decide** need the user's decision first.

### macOS signing and notarisation

- Sign with a **Developer ID Application** certificate. Tauri signs and notarises when
  these are set:
  - `APPLE_CERTIFICATE`: the base64 `.p12`;
  - `APPLE_CERTIFICATE_PASSWORD`;
  - `APPLE_SIGNING_IDENTITY`;
  - for notarisation, an App Store Connect API key: `APPLE_API_ISSUER`,
    `APPLE_API_KEY` and the key file.
  `bundle.macOS.signingIdentity` changes from `"-"` to the Developer ID identity (or
  comes from the environment).
- **Where the secrets live:** a GitHub **environment** named `release`.
  - It requires approval by the user before a job can use it.
  - Only the macOS build job of a tag build uses it.
  - Pull requests and dry runs never see the secrets. The repository is public, and
    fork pull requests already need approval to run CI at all.
- Hardened runtime is already on. Start with no extra entitlements and add only what
  the app is shown to need (for example, check whether WebKit needs
  `com.apple.security.cs.allow-jit`).
- New checks in the workflow:
  - `codesign --verify --deep --strict`;
  - `spctl --assess --type execute -vv` (Gatekeeper accepts it);
  - `xcrun stapler validate` on the app and the disk image.

### Linux

An AppImage has no platform signing.
- **Decide:** sign `SHA256SUMS` with a project key (GPG or minisign), or rely on GitHub
  build-provenance attestations (keyless, through the workflow's OIDC token; no key to
  keep), or both.
- **Decide:** build on an older base (for example Ubuntu 22.04) so the AppImage runs on
  older distributions.

### The model check

The local model (T2.3) is downloaded after install. fm-llm already pins each model by
URL, size and SHA-256. It hashes the file while it streams, and only a verified file
reaches its final path (`crates/fm-llm/src/download.rs`).
- **Plan:** keep the list of allowed models (URL, size, SHA-256) inside the app's
  binary. Signing the app then covers the model list too, and a model that doesn't
  match is never loaded, wherever it came from.
- **Decide:** whether model updates should also be possible without an app release.
  That would need a signed model list with its own key, kept offline.

### Update checks

- **Decide:** whether the app checks for updates at all, and whether that is on by
  default or opt-in. A check reveals to GitHub that someone runs the app, and SPEC §7
  limits what leaves the machine.
- Tauri's updater plugin brings its own HTTP client. That conflicts with SPEC decision
  21 (every request through fm-netguard) and `deny.toml` (network crates only in
  fm-netguard).
- **Plan:** a small update check through fm-netguard. It fetches a release manifest
  from the GitHub origin only (allowlisted for the check) and verifies the manifest's
  signature against a public key built into the app. It then tells the user a new
  version exists and links to the release, with no silent download or install.
- The manifest is signed with an Ed25519 (minisign) key kept offline. Its public key
  goes in the app; the signed manifest is attached to each release.
- `bundle.createUpdaterArtifacts` stays `false` unless Tauri's updater is adopted.
