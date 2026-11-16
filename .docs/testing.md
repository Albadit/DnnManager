# Testing

How DNN Manager is tested, what was checked in the running app, and what wasn't yet.

## Automated tests

```powershell
cargo test --workspace        # 819 tests (12 ignored), a few seconds once built
cargo clippy --workspace --all-targets
```

None needs IIS, SQL Server, Docker or administrator rights. Tests that write use a temporary folder, never your
`dnnmanager.db`, the hosts file, the Credential Manager or a scheduled task. The ignored ones need a real service and
are run by hand.

| Crate | Tests | What they hold |
|---|---|---|
| `dnnm-application` | 166 | The use cases against stand-in traits: their steps in order, messages word for word, undo last-first on a failure or a cancel, Keep, the questions they ask; settings rows and defaults; row texts |
| `dnnm-dialogs` | 109 | The dialogs' and pages' rules as pure functions: settings validation and categories, shortcuts, the upgrade path file, the setup page's checks, Markdown, the guide and tour stops, release notes between versions |
| `dnnm-dnn` | 105 (2 ignored) | DNN packages and releases, the installer's web.config edits, upgrade path and plan |
| `dnnm-system` | 81 (1 ignored) | Hosts file, ACLs, secrets (DPAPI round trip), sign-in task arguments, updates and the update helper's plan, single instance |
| `dnnm-sql` | 77 (3 ignored) | Connection strings, LocalDB, SQL texts, docker-compose, the provisioner's checks |
| `dnnm-wiring` | 54 (1 ignored) | The real adapters in temporary folders: zip round trips, web.config rewrites, backups and their clean-up, setup drafts |
| `dnnm-infrastructure` | 53 | appcmd's XML and errors, web.config, `dnnmanager.db` (its schema, changed rows only), Windows calls |
| `dnnm-logs` | 44 (1 ignored) | Log catalogs, the daily file, the run log, Find |
| `dnnm-terminal` | 38 | Shell discovery, the VT screen |
| `dnnm-keepwarm` | 28 | Ping rules and intervals, statuses |
| `dnnm-openwith` | 27 (3 ignored) | Editors and SSMS found, the menu's entries, the clipboard secret |
| `dnnm-diagnostics` | 23 | The Details tabs and the health checks |
| `dnnm-domain` | 5 | Project name rules |
| `dnnm-tray` | 5 (1 ignored) | The icon's image and tooltip; the ignored one adds and removes a real icon |
| `dnnmanager` | 4 | Column keys and defaults |

## The stand-in IIS

Reading IIS needs administrator rights, so the UI is checked against a stand-in IIS
([`iis_fixture.rs`](../crates/infrastructure/src/iis_fixture.rs)): `DNNMANAGER_GPUI_FIXTURE` names a file
of appcmd's site XML, the app shows those sites, and Start / Stop change only its in-memory copy. Point the sites'
`physicalPath` at real DNN folders and the DNN version and database come from them - the folders are only read.

```xml
<appcmd>
  <SITE SITE.NAME="dnn10" SITE.ID="2" state="Started"><site name="dnn10" id="2">
    <bindings><binding protocol="http" bindingInformation="*:80:dnn10.dnndev.me" /></bindings>
    <application path="/" applicationPool="dnn10"><virtualDirectory path="/" physicalPath="C:\DNN\dnn10" /></application>
  </site></SITE>
</appcmd>
```

```powershell
$env:DNNMANAGER_GPUI_FIXTURE = "C:\path\to\sites.xml"; .\target\debug\DnnManager.exe
```

The stand-in refuses every operation that changes something (Remove, Clone, Upgrade, Troubleshoot's clean-up…) with
a toast, and doesn't add the tray icon, delete expired backups or write the onboarding state at the start. It still
uses the real `dnnmanager.db`: changing the theme, the columns or the settings there changes your settings, and New
project saves what is typed as a draft.

## Checked in the running app

On Windows 11 at 1280 × 820, dark theme, with the stand-in IIS, by screenshots - no pixel-difference tool was run.
These checks were made while the app was ported from the earlier C# version, against that version's look.

| Check | Result |
|---|---|
| Real IIS, elevated (once) | All 10 sites, addresses, DNN versions and databases read correctly |
| Projects table: columns, frozen columns, sideways scrollbar, Columns popup, search, Stop a site, keep warm flames | Work; Stop changed the stand-in's copy and wrote to Output |
| Command palette, Go to project, palette pick, Customize Layout, Ctrl+, , Ctrl+J, Output / Logs / Terminal tabs | Work |
| Gear menu | Nine entries, Themes as a submenu, shortcuts on the right; beside the gear, bottom level with it |
| Settings | Nine categories; General and Keyboard shortcuts browsed; every command listed with its default shortcut |
| Troubleshoot (from the palette) | Folder sizes measured; its buttons left alone (the stand-in refuses the clean-up and resets) |
| New project | Every card drawn; 72 releases loaded from GitHub; text wraps (two layout bugs found and fixed: a label wrapping per letter, a hint leaving a gap) |
| Help menu, F1 page help, guided tour | Menu names the page; F1 opened "Help: Project details"; the tour rang the sidebar, then switched to Projects and rang the table |
| Row menu | Every entry; Restore backup ▸ filled ("No backup…" for a site without one) |
| Clone, Upgrade DNN, Edit host names dialogs | Opened with the site's values (`dashboard_copy` suggested; "nothing newer" for 10.4.1); Esc closes them (it used to clear the field - fixed) |
| Details | General, IIS, DNN tabs with the database's own facts; ⋮ More actions opens the row menu; tables sized to their text (two bugs found and fixed: "Reading…" for ever when the database couldn't be read, columns overlapping) |
| Check for Updates | Asked GitHub: "Up to date - v1.8.7 is the newest release" |

## Packages and other systems

- The .msi was built on this PC with cargo-packager (see [releasing.md](releasing.md#a-package-on-this-pc)); the
  .dmg and .deb are built only by the release workflow.
- macOS and Linux: the workspace is type-checked for Linux and (without GPUI) macOS on Windows; CI builds and tests
  it there. The app hasn't been started on either system by hand.

## Not checked

- **Every use case that changes something** - create, import, clone, remove, backup, restore, export for deployment,
  upgrade, rename, bindings, app pool, database connection, host password, clear cache, the SQL container, IIS
  features, Install Docker: the stand-in IIS refuses them, and running them needs real IIS and SQL Server on a PC where a
  lost site doesn't matter. They are covered by their tests only.
- Start / Stop / Restart against real IIS, and `iisreset`.
- Saving Settings, switching the theme, recording a shortcut, Discard - each writes the real, shared settings.
- The tray icon, hiding on close, the quit questions, Restart, the guide and What's new at the start - off with the
  stand-in IIS, because they would change the system or the shared state.
- The dialogs not opened: rename, app pool, database connection, host password, export for deployment, upgrade path,
  IIS features, What's new.
- The light theme, the Compact density, a narrow window, the sidebar on the right, other font sizes.
- Snap layouts on the maximize button; keyboard-only use; screen readers.
- No end-to-end test suite: GPUI has a test harness (`gpui::TestAppContext`), not yet used here.
