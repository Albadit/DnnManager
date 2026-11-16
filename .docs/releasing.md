# Releasing

A release is built, tested and published by GitHub from a version tag - nothing built on a PC is uploaded, and
nothing is published unless every build and test passed.

```mermaid
flowchart LR
    A["VS Code: Release: Github<br/>1 type, 2 version, 3 commit, 4 confirm"] --> B["tag vX.Y.Z pushed"]
    B --> C["plan"]
    C --> D["test<br/>Windows, Linux, macOS"]
    D --> E["package<br/>6 installers"]
    E --> F["publish<br/>draft, check, publish"]
```

## Steps

1. **Release notes** (optional): `.docs/release-notes/vX.Y.Z.md`, committed and pushed (the `release-notes` skill
   knows the structure). The release gets it as its notes and the app shows it under **What's new**; without it GitHub
   writes notes from the commits.
2. **Run the VS Code task Release: Github** (Ctrl+Shift+B). Four pickers, each filled by `scripts/release.ps1`:

   | Picker | What it offers |
   |---|---|
   | 1/4 Release type | **Latest** (what DNN Manager's update check finds), **Pre-release**, **None** |
   | 2/4 Version | The versions whose notes are in `.docs/release-notes/` and that aren't released yet, of the type picked (`v2.0.0-rc2.md` → 2.0.0-rc2). Without any, the next patch, minor and major after the newest release on GitHub, marked *no release notes yet* (a pre-release gets the next free `rcN`). Any other can be typed. |
   | 3/4 Commit | The newest 30 commits on the branch's remote - a release is made from a pushed commit only; a released one says so |
   | 4/4 Confirm | The release spelled out (version, type, commit, notes) - or, when something is wrong, only *Can't release* with the reason |

   It refuses - before anything is pushed - a version that isn't newer than the newest release, a tag that exists, a
   pre-release without a suffix (`-rc1`) or a Latest with one, and a commit that isn't on the remote. On **Release** it
   checks all of that again, tags the commit (an annotated tag whose message carries `Release-Label: <type>`) and
   pushes only the tag.
3. **Follow the run** on GitHub (Actions → Release). The `publish` job runs in the environment `publish`: give that
   environment a required reviewer in the repository's settings and every release waits for approval.

The version isn't kept in the repository: the tag is the version. The workflow writes it into `Cargo.toml`,
`Cargo.lock` and `Packager.toml` before building (`scripts/set-version.py`; `Packager.toml` gets the plain X.Y.Z, as an
MSI's version is numbers only). The `1.8.7` in `Cargo.toml` is only what a local build says.

## What the workflow does

`.github/workflows/release.yml`, on a pushed tag `vX.Y.Z` or `vX.Y.Z-suffix`:

| Job | Runs on | What it does - a failure stops everything after it |
|---|---|---|
| plan | ubuntu-24.04 | The tag is a version; the release type from the tag's message (a tag without one: Latest, or a pre-release with a suffix); type and suffix agree; the tag isn't published already |
| test | windows-2025, ubuntu-22.04, macos-14 | Builds everything, clippy and every test - warnings are errors |
| package | one runner per installer | Writes the version in, builds clean (`-D warnings`), packages with cargo-packager, checks the file is there and not empty. Only the installer leaves the job. |
| publish | ubuntu-24.04, environment `publish` | Checks the files are exactly the six installers (never an executable), adds the .debs as `-x86_64.deb` / `-aarch64.deb` and `SHA256SUMS.txt`, uploads everything to a **draft**, checks every file arrived, then publishes it with its type. A failure deletes the draft - no release is left half made. |

| Installer | Runner |
|---|---|
| `DnnManager-windows-x86_64.msi` | windows-2025 |
| `DnnManager-windows-arm64.msi` | windows-11-arm |
| `DnnManager-macos-arm64.dmg` | macos-14 |
| `DnnManager-macos-x86_64.dmg` | macos-14 (cross-built) |
| `DnnManager-linux-amd64.deb` (also `-x86_64.deb`) | ubuntu-22.04 |
| `DnnManager-linux-arm64.deb` (also `-aarch64.deb`) | ubuntu-22.04-arm |

[cargo-packager](https://github.com/crabnebula-dev/cargo-packager) (version pinned in the workflow) makes them from
`Packager.toml`: WiX for the .msi, a disk image for the .dmg, a Debian package for the .deb (its run-time libraries
listed under `[deb] depends`). The Linux jobs install GPUI's build dependencies with `scripts/linux-deps.sh`; Ubuntu
22.04 keeps the .deb working on older systems.

The installers are **not code-signed**. Signing needs a certificate (Windows) and an Apple Developer ID (macOS) stored
as repository secrets - not set up.

## A build on this PC

The VS Code task **Publish** (`scripts/publish.ps1`) asks **executable** or **installer**, then which systems -
several at once (the multi-select picker comes from the extension **Tasks Shell Input**,
`augustocdias.tasks-shell-input`, which the workspace recommends; the list, with its icons, is written in `.vscode/tasks.json`, input `publishPlatforms`):

| Pick | Into `dist\` |
|---|---|
| executable | `DnnManager-windows-x86_64.exe`, `DnnManager-linux-amd64`, … (Linux also as `linux-x86_64` / `linux-aarch64`, the release's other names) |
| installer | the executable and its `.msi` / `.dmg` / `.deb`, e.g. `DnnManager-windows-x86_64.msi` |

Every build uses `-D warnings`, so a warning stops it. A PC compiles for its own operating system
only: Windows for `windows-x86_64` and `windows-arm64` (that one needs
Visual Studio's *MSVC ARM64 build tools*), a Mac for both macOS builds, Linux for Linux. Systems picked that this PC
can't build are skipped with a note - the release on GitHub builds all of them. Without the task (no
`-Platform`: it lists the systems in the terminal and asks):

```powershell
powershell -File scripts/publish.ps1 -Kind installer -Platform windows-x86_64,windows-arm64
```

An installer needs cargo-packager once: `cargo install cargo-packager --locked`. It takes the executable from its
output folder, so the script copies it there first. Tried on Windows: `dist\DnnManager-windows-x86_64.msi` (about
12 MB), which installs `DnnManager.exe` into `Program Files\DNN Manager`.

## Release a version again

The tag exists but its run failed (a test, a package), so nothing was published. Fix it, push the fix, and either
release the next version, or move the tag to the new commit:
`powershell -File scripts/release.ps1 -Label <type> -Version X.Y.Z -Commit <sha> -Confirm yes -Redo`. `-Redo` refuses
a tag whose release is published - DNN Manager offers only newer versions, so a published release is never redone.

## Updates inside the app

**Check for Updates** asks GitHub for the newest release and says whether there is a newer one
([`updates.rs`](../crates/app/src/workspace/updates.rs)). Installing it is done by downloading the package from the
release page.
