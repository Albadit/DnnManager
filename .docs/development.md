# Development

How to build, run and debug DNN Manager, where code goes, and the rules every part follows. The big picture:
[architecture.md](architecture.md).

## Set up

| System | Needs |
|---|---|
| Windows 10/11 | Rust (rustup) and Visual Studio or the Build Tools with **Desktop development with C++** and a **Windows 11 SDK** - GPUI compiles its shaders with the SDK's `fxc.exe` (or set `GPUI_FXC_PATH`). [`rust-toolchain.toml`](../rust-toolchain.toml) picks `stable-x86_64-pc-windows-msvc`: GPUI doesn't build with the GNU toolchain. |
| macOS | Rust and Xcode's command line tools. Override the toolchain file: `RUSTUP_TOOLCHAIN=stable`. |
| Debian / Ubuntu | Rust and GPUI's libraries: `bash scripts/linux-deps.sh`. Override the toolchain file: `RUSTUP_TOOLCHAIN=stable`. |

## Build, run, test

```powershell
cargo build -p dnnmanager                 # target\debug\DnnManager.exe (first build several minutes)
cargo run -p dnnmanager                   # build and start
cargo build --release -p dnnmanager       # optimised, no console window
cargo test --workspace                    # every crate's tests - no IIS, SQL Server or admin rights
cargo clippy --workspace --all-targets
```

The VS Code tasks (Ctrl+Shift+B lists them: **Build: Debug**, **Build: Release**, **Run**, **Test**, **Clippy**,
**Package**, **Publish**, **Release: Github**) run the same. Build into a
folder of its own (`--target-dir <folder>`) when the IDE is building too.

On Windows DNN Manager needs administrator rights to read IIS's configuration: started without them, it starts
itself again with the UAC prompt; refused, it runs anyway and the Projects page says why it can't read IIS.

| Environment variable | Effect |
|---|---|
| `DNNMANAGER_GPUI_NO_ELEVATE` | Set to anything: don't ask for administrator rights (IIS can't be read). For working on the UI. |
| `DNNMANAGER_GPUI_FIXTURE` | A file of appcmd's site XML: show those sites instead of IIS's, and refuse every operation that changes something. See [testing.md](testing.md#the-stand-in-iis). |

Both still use the real `Documents\DnnManager\dnnmanager.db`: settings changed there are your settings.

## Debugging

- DNN Manager's log: `Documents\DnnManager\logs\dnnmanager-<yyyymmdd>.log`, also in the app under **Logs → DNN
  Manager**. Only DNN Manager's own lines go there (not GPUI's).
- An operation's steps and its error: the **Output** tab; each run is also written to the log.
- A debug build keeps its console window: `log::` lines and panics show there.
- An operation that was running when DNN Manager ended is reported at the next start (state area `operation`).

## Where code goes

| What | Where |
|---|---|
| Records and rules with no dependencies | `crates/domain` |
| A use case, its steps and the traits it sees the system through | `crates/application/src/use_cases/<name>.rs` |
| The use case's traits implemented with the real system, and one entry point for the app | `crates/wiring/src/<name>.rs`: `pub fn <name>(services: &Services, request, op: &mut Operation) -> Result<(), String>` |
| IIS (appcmd), web.config, `dnnmanager.db`, programs, paths | `crates/infrastructure` |
| A subsystem: SQL Server, DNN releases, Windows services, keep warm, logs, terminal, Open with, diagnostics, the tray | `crates/<name>` (package `dnnm-<name>`) - the workspace picks up every folder under `crates/` |
| A dialog or a page other than Projects | `crates/dialogs/src/<name>.rs`, a GPUI view emitting `DialogEvent<T>` (or its own events), drawn with `dnnm_ui` |
| Theme, controls, menus, the text field | `crates/ui` |
| The window, the Projects page, the store, menus and commands | `crates/app` |

A crate may use the ones below it ([architecture.md](architecture.md#crates-and-layers)); `dnnm-application` uses
only `dnnm-domain`, so the use cases can be tested without the system.

## Operations

Everything that changes the system is a use case run as an operation
([`operation.rs`](../crates/application/src/operation.rs)):

```rust
pub fn remove_project(deps: &RemoveDeps, request: &RemoveRequest, op: &mut Operation) -> Result<(), String> {
    op.report.plan(&["Remove IIS site", "Drop database", "Delete files"]);
    op.cancel.check()?;                      // between steps: Cancel ends the operation
    op.report.step("Remove IIS site");
    if !op.prompt.confirm("Remove 'x'? …", "Remove", "Cancel", true) {
        return Err(operation::CANCELLED.into());
    }
    op.undo.add("Put the site back", operation::STEP_LIMIT, move |cancel| { /* … */ Ok(()) });
    op.undo.cannot_undo("the database [x]"); // what can't be taken back
    Ok(())
}
```

- Note the undo **before** the step runs; on a failure or a cancel the undo runs, last step first. `op.undo.keep()`
  leaves everything for the user to look into.
- Long work checks `op.cancel` and passes it on to the programs it runs.
- Report with `op.report.step / info / success / warn / fail`; questions with `op.prompt`, which become the window's
  dialog while the operation's thread waits.
- The app runs it with `Store::run_changing` (refused while another runs, and in the fixture) or `run_then` (with a
  follow-up when it ends).

## Rules

DNN Manager runs elevated; what the user's account can change is untrusted input.

- **Programs**: only `dnnm_infrastructure::process::run` (System32 programs, argument lists, no shell, a time limit)
  or a program whose path only administrators can change. Never a command line built from strings.
- **Paths** in folders others can write (a site, the projects folder, Documents): compare with
  `web_config::is_inside`, check for links before a write or delete, never follow a link or junction.
- **Zip files**: entry by entry, refuse entries outside the target and check sizes first.
- **XML**: `web_config::load_site_xml` refuses a DTD.
- **SQL**: values as parameters; a name in SQL text quoted as `[name]` with `]` doubled. Passwords never in a command
  line, a log line, an error message or the UI.
- **Secrets**: Credential Manager or DPAPI; never on disk in clear.
- **Temporary files** read back or run: a folder only administrators can write, not `%TEMP%`.
- **Windows only**: code that calls Windows APIs is behind `cfg(windows)`, with a non-Windows side that says the
  feature needs Windows - the workspace builds on macOS and Linux too (CI checks it).
- **Tests** never change the system: no IIS writes, SQL Server, Docker, hosts file, Credential Manager or the user's
  `dnnmanager.db` - a temporary folder or a stand-in trait; a test that needs a real service is `#[ignore]`d with a
  comment saying what it needs.
- **Spacing**: no margins and no per-side padding - gaps between elements, one `.p(..)` / `.pad(v, h)` inside a
  container ([design-system.md](design-system.md#spacing)).
- **Files**: UTF-8 without BOM, CRLF (shell scripts LF); `rustfmt.toml` sets 120 columns. Comments say why, not what.
  User-visible texts are complete sentences in plain words.
