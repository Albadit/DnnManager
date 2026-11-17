---
name: release-notes
description: >-
  Writes the release notes for the next DNN Manager release as .docs/release-notes/vX.Y.Z.md (bold summary,
  Highlights, Other changes, Upgrading, Tested), from the commits since the previous tag and real test results - every
  claim checked against the code - and the commit message for the notes, ready to paste. The release workflow publishes that file as the GitHub release's notes and What's new shows
  it in the app. Use when asked to write, draft, update or review release notes, prepare a release or "the next
  release" - and to redo a release whose tag is already made (a failed release workflow, one more change before
  anyone has it).
---

# Release notes

The notes for a release are **`.docs/release-notes/vX.Y.Z.md`**. They are used twice:

- the release workflow (`.github/workflows/release.yml`) publishes the file as the GitHub release's notes (without
  it, GitHub writes notes from the commits);
- the app embeds every `.docs/release-notes/v*.md` at build time (`crates/dialogs/build.rs`) for **What's new**.

So the file is written and committed **before** the tag is pushed. Readers are **people who use DNN Manager**,
deciding whether to update and what changes for them - not developers.

## Workflow

1. **The version.** The one asked for; when none is, ask. The previous release is the newest tag:
   `git describe --tags --abbrev=0`. When the version's tag exists already, this is a **redo** - see below.
2. **The version comes from the tag**, picked in the task **Release: Github** (it suggests the next patch, minor and
   major after the newest tag); the workflow writes it into the build. Nothing in `Cargo.toml` needs raising.
3. **Collect what changed**: `git log --no-merges --format="%h %s%n%b" <previous tag>..HEAD`, then the diff
   (`git diff --stat <previous tag>..HEAD`, and the files) wherever a commit is unclear.
4. **Verify every claim against the code**: UI labels exactly as in the Rust source (`crates/app`, `crates/dialogs`),
   settings keys and defaults, paths, numbers. Drop what you can't confirm, or ask.
5. **Get the test results**: `cargo test --workspace` and the counts; compare with the previous notes' *Tested*.
   Manual checks only if they were really done - ask the owner what was tested by hand, and on which systems
   (Windows with IIS, macOS, Linux).
6. **Write `.docs/release-notes/vX.Y.Z.md`** in the structure below; the file name is exactly `v` + the version
   (`v2.0.0.md`, `v2.0.0-rc1.md` for a pre-release).
7. **Write the commit message** (below) and give it in the report.
8. Report what the notes say, the commit message and what is unverified. Don't commit, tag or publish: the owner
   commits and pushes the notes, then runs the VS Code task **Release: Github** (type, version, commit, confirm), which
   tags the commit and pushes the tag; GitHub tests, builds and publishes only when everything passed.

## Redoing a release

The tag exists but its run failed, so nothing was published. Update the notes in place, have the owner commit and
push, then run **Release: Github** again with the same version and the new commit: only a published release counts
as released, so the picker still offers it and the release moves the tag. A published release is refused - DNN
Manager offers only *newer* versions; recommend the next one then.

## Structure

```markdown
# DNN Manager X.Y.Z

**The release in a few words.** One or two sentences on what it does for the user.

## Highlights

### A theme, named by what the user gets
- What they can do now, and where (**UI name** in bold).

## Other changes
- Smaller changes, one line each.
- Fixed: a bug, described by what the user saw.

## Upgrading
Install the new package over the old one: the .msi on Windows, the .dmg on macOS, the .deb on Linux. Your settings
keep their format.
- Anything the user must do once, or that now works differently.

## Tested
- What was checked by hand, on which system, against what.
- All N automated tests pass, up from M.
```

| Part | Rules |
|---|---|
| **Title** | `# DNN Manager X.Y.Z`, then the summary - no date or download links (GitHub lists the files). |
| **Summary** | A bold phrase of 3-7 words, then 1-2 plain sentences. No version number, no "This release…". |
| **Highlights** | 3-6 `###` themes, the most important first; 2-7 bullets each, by what the user gets. |
| **Other changes** | One line each; bug fixes last, as `Fixed: …`. |
| **Upgrading** | Always present. Bullets only for actions or changed behaviour. |
| **Tested** | Only real results; leave it out rather than guess. |

## Style

- "You", present tense, what they can do and where. UI elements exactly as on screen, in **bold**; files, settings
  and commands in `code`.
- Concrete over vague; numbers only when measured. No commit hashes, type names or dependency bumps unless they
  change what the user sees. No emoji or marketing words.
- UTF-8 without BOM, CRLF line endings.

## Commit message

One line, no body:

```text
release: vX.Y.Z - <the Highlights' themes, lower case, comma-separated>
```
