# DNN Manager - notes for Claude

## Skills

This repository has twelve skills in `.claude/skills/`. For every prompt, decide which of them the request needs, use
each one that applies, and name them in the first line of the reply (e.g. "Skills: functionality, documentation" or
"Skills: none").

**Code: one specialist per quality area.** A task that is mainly about one area loads that one skill; a review, an
audit or work that spans several areas goes through **orchestrator**, which scores the nine areas, runs the ones
that matter and merges their findings.

| Skill | Use it when the prompt… |
|---|---|
| **functionality** | builds a feature, fixes a bug, writes tests - business logic, edge cases, state, error paths |
| **security** | touches elevation, credentials, SQL, the hosts file, downloads and updates, file and zip handling |
| **reliability** | is about failure and recovery: undo/rollback, timeouts, retries, cancel and quit, partial state |
| **performance** | is about speed or resource use: UI-thread work, the monitor, logs, copies, memory |
| **usability** | changes the UI or its wording: flows, error messages, keyboard, contrast, screen readers |
| **maintainability** | refactors or cleans up: duplication, long methods, naming, tests, tech debt |
| **architecture** | changes structure: layers, abstractions, module boundaries, ADRs |
| **compliance** | handles personal data or secrets at rest: backups, exports, logs, the clipboard |
| **operations** | touches CI, the release workflow, the packages (`Packager.toml`), self-update, logging or settings storage |
| **orchestrator** | asks for a review, an audit or "run all skills", or carries `#all`, `#eco`, `#sync`, `#fix`, `#deeper` or a dimension tag such as `#security` |

**Docs and releases.**

| Skill | Use it when the prompt… |
|---|---|
| **documentation** | writes or updates docs: README, `.docs/`, diagrams, a handover - or a code change alters documented behaviour, configuration, setup or architecture |
| **release-notes** | writes or reviews the notes for a release (`.docs/release-notes/vX.Y.Z.md`) - "the release notes for 2.0.0", "prepare the next release" - or redoes a release whose tag is made (a failed release workflow) |

A code skill and documentation apply together when a code change touches something the docs describe. None of them
applies to a quick question or a one-off command.

## Project conventions

- DNN Manager is a Rust + GPUI app: a Cargo workspace at the root (`crates/*`), the window in `crates/app`. The C#/WPF
  app it was ported from is gone from the tree (git history before the Rust move has it).
- Files are UTF-8 without BOM with CRLF line endings (`.editorconfig`); shell scripts are LF (`.gitattributes`). Don't
  edit files with `sed -i` - it can drop the CRs.
- Build into a separate folder when the IDE may be building too: `cargo build -p dnnmanager --target-dir <temp folder>`.
- Tests: `cargo test --workspace` (no IIS, SQL Server or administrator rights needed).
- The UI can be tried against a stand-in IIS: `DNNMANAGER_GPUI_FIXTURE` - see [.docs/testing.md](.docs/testing.md).
- Packages: `.msi` (Windows), `.dmg` (macOS), `.deb` (Linux) by cargo-packager (`Packager.toml`), built and published by
  `.github/workflows/release.yml` when a version tag is pushed - [.docs/releasing.md](.docs/releasing.md).
- The owner commits; don't commit or push unless asked.
- Architecture, conventions and how to debug: [.docs/development.md](.docs/development.md).
