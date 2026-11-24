# Testing

How DNN Manager is tested, what was checked in the running app, and what wasn't yet.

## Automated tests

```powershell
cargo test --workspace        # 952 tests (12 ignored), a few seconds once built
cargo clippy --workspace --all-targets
```

None needs IIS, SQL Server, Docker or administrator rights. Tests that write use a temporary folder, never your
`dnnmanager.db`, the hosts file, the Credential Manager or a scheduled task. The ignored ones need a real service and
are run by hand.

| Crate | Tests | What they hold |
|---|---|---|
| `dnnm-application` | 215 | The use cases against stand-in traits: their steps in order, messages word for word, undo last-first on a failure or a cancel, Keep, the questions they ask; settings rows and defaults; row texts; Source Control against an in-memory project and server that loses its connection, refuses a path or changes behind the comparison; 20 000 files compared within a time limit |
| `dnnm-dialogs` | 124 | The dialogs' and pages' rules as pure functions: settings validation and categories, shortcuts (each OS's text, macOS's ⌘ defaults), the upgrade path file, the setup page's checks, Source Control's list and environment form, Markdown, the guide and tour stops, release notes between versions |
| `dnnm-dnn` | 105 (2 ignored) | DNN packages and releases, the installer's web.config edits, upgrade path and plan |
| `dnnm-system` | 86 (1 ignored) | Hosts file, ACLs, secrets (AES-256-GCM with stand-in keys, reading old DPAPI values), sign-in task arguments, updates and the update helper's plan, single instance |
| `dnnm-sql` | 77 (3 ignored) | Connection strings, LocalDB, SQL texts, docker-compose, the provisioner's checks |
| `dnnm-wiring` | 54 (1 ignored) | The real adapters in temporary folders: zip round trips, web.config rewrites, backups and their clean-up, setup drafts |
| `dnnm-infrastructure` | 66 | appcmd's XML and errors, web.config, `dnnmanager.db` (its schema, changed rows only), Source Control's folder scan, writes and tables, Windows calls, the status bar's memory / processor / disk (read for real on the OS the tests run on) |
| `dnnm-logs` | 44 (1 ignored) | Log catalogs, the daily file, the run log, Find |
| `dnnm-terminal` | 39 | Shell discovery, the VT screen, each OS's copy and paste keys |
| `dnnm-ui` | 39 | Text field editing for each OS (words, undo, IME, selection) and its key tables, how each OS writes a shortcut, icons, tips, selects |
| `dnnm-keepwarm` | 28 | Ping rules and intervals, statuses |
| `dnnm-openwith` | 27 (3 ignored) | Editors and SSMS found, the menu's entries, the clipboard secret |
| `dnnm-diagnostics` | 23 | The Details tabs and the health checks |
| `dnnm-domain` | 5 | Project name rules |
| `dnnm-tray` | 5 (1 ignored) | The icon's image and tooltip; the ignored one adds and removes a real icon |
| `dnnmanager` | 4 | Column keys and defaults |
| `dnnm-remote` | 4 | SFTP: an untrusted server is never signed in to; FTP listings in both forms; the TLS settings; with `DNNM_SFTP_TEST` / `DNNM_FTP_TEST` / `DNNM_FTPS_READONLY` set, real servers end to end (below) |

Source Control against real servers - these tests pass without doing anything unless `DNNM_SFTP_TEST` or
`DNNM_FTP_TEST` names one. With Docker:

```powershell
docker run -d --name dnnm-sftp-test -p 127.0.0.1:2222:22 atmoz/sftp tester:secret:::upload
docker run -d --name dnnm-ftp-test -p 127.0.0.1:2121:21 -p 127.0.0.1:30000-30009:30000-30009 -e PUBLICHOST=127.0.0.1 -e FTP_USER_NAME=tester -e FTP_USER_PASS=secret -e FTP_USER_HOME=/home/tester stilliard/pure-ftpd
$env:DNNM_SFTP_TEST = "127.0.0.1:2222:tester:secret:/upload"; $env:DNNM_FTP_TEST = "127.0.0.1:2121:tester:secret:/"
cargo test -p dnnm-remote -- --test-threads=1
docker rm -f dnnm-sftp-test dnnm-ftp-test
```

A Docker FTPS server's self-signed certificate is (rightly) refused, so FTPS is checked against a real site, read
only: `live_read_only.rs` signs in, compares the whole folder with an empty project, reads a file's facts, previews
and downloads five small files, compares again, shows the cached comparison and reads both copies for the diff -
nothing on the server is changed. For an Azure App Service, with the FTPS details from its publish profile:

```powershell
$env:DNNM_FTPS_READONLY = 'waws-prod-xx-000.ftp.azurewebsites.windows.net:21:app\$app:<password>:/site/wwwroot'
cargo test -p dnnm-remote --test live_read_only -- --nocapture
```

`real_server.rs` streams, replaces and removes a file and checks a wrong host key is refused; `source_control_end_to_end.rs`
runs compare → upload → change on each side and both → diff → upload, download, keep the server's side → delete on
the server, through the real SFTP client, project folder and tables.

## The stand-in IIS

Reading IIS needs administrator rights, so the UI is checked against a stand-in IIS
([`iis_fixture.rs`](../crates/infrastructure/src/iis_fixture.rs)): `DNNMANAGER_GPUI_FIXTURE` names a file
of appcmd's site XML, the app shows those sites, and Start / Stop change only its in-memory copy - taking two seconds each, as a real IIS does, so the Output's Running state (the spinner, the clock) can be seen. Point the sites'
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
