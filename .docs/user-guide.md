# User guide

DNN Manager runs DNN websites on this PC's IIS. This guide covers installing it, the pages, and what to do when
something goes wrong. Inside the app, **F1** shows help for the page you are on, and **? → Getting Started Guide**
and **Take the Tour** walk you through the window.

## Install

Download the package for your system from the [releases](https://github.com/Albadit/DnnManager.NET/releases):

- **Windows**: `DnnManager-windows-x86_64.msi` (or `-arm64.msi` on an ARM PC). Run it; DNN Manager is then in the
  Start menu. The package isn't code-signed yet, so SmartScreen may ask first - **More info → Run anyway**.
- **macOS**: open the `.dmg` and drag DNN Manager to Applications. The package isn't signed by Apple, so the first
  start is blocked: macOS says it can't check DNN Manager. Choose **Done**, then **System Settings → Privacy &
  Security → Open Anyway**. If macOS says instead that DNN Manager **"is damaged and can't be opened"** (releases
  before 2.0.0-rc3 do), the app is fine: run this once in Terminal, then open it as usual:

  ```sh
  xattr -dr com.apple.quarantine "/Applications/DNN Manager.app"
  ```
- **Debian / Ubuntu**: `sudo apt install ./DnnManager-linux-amd64.deb` (or `-arm64.deb`).

To update, install the newer package over the old one. **Settings → About** and **Check for Updates** (the gear
menu) say whether there is a newer release.

## What you need on Windows

- **IIS** with ASP.NET 4.x. **Settings → IIS** lists the Windows features DNN needs and **Set up IIS** turns them on.
- **SQL Server** for the sites' databases: the Docker container DNN Manager sets up (**Settings → Docker container**,
  needs Docker Desktop), or a SQL Server you already have (**Settings → Database server**, then **Test**).
- **Administrator rights**: IIS's configuration can only be read and changed by an administrator, so DNN Manager asks
  for them (the UAC prompt) when it starts.

## The window

| Part | What it does |
|---|---|
| Activity bar | The icons on the left: **Projects** (every DNN website in IIS), **New project** and **Source Control**; the gear opens Settings, themes, Customize Layout, Troubleshoot and Check for Updates. Customize Layout hides it or puts it on the right. |
| Side bars | A **primary** side bar beside the activity bar (**Ctrl+B**) and a **secondary** one on the other side (**Ctrl+Alt+B**). On the Source Control page the primary side bar holds its files (it opens when you go there); elsewhere the side bars are empty for now, kept for what comes later. Drag their edges to resize them. |
| Title bar | The search in the middle finds a project; type `>` for any command (**Ctrl+Shift+P**). **?** is help. |
| Bottom panel | **Output** (what an operation does, step by step), **Logs** (the sites' and DNN Manager's logs), **Terminal**. **Ctrl+J** opens and closes it; drag its top edge to resize it (it stops at its smallest size), or the corner where it meets the primary side bar to resize both. **Ctrl+F** (or 🔍) finds text in the tab shown: **Aa** match case, **ab** whole word, **.\*** regular expression; **Enter** / **Shift+Enter** go to the next / previous match, **Esc** closes it. The text in Output and Logs wraps in the panel's width and can be selected: drag over it, double-click a word, triple-click a line; **Ctrl+C** (⌘C) copies, **Ctrl+A** (⌘A) selects all, and the right-click menu has both. |
| Status bar | IIS's state with Start / Stop / Restart (these affect every site), memory, processor and the disk of the projects folder. A figure that can't be read shows dashes (`--.--`), and processor use shows from its second reading on, two seconds after the start. |

**Without a mouse**: **Tab** and **Shift+Tab** go from control to control, **Enter** or **Space** presses the one
with the blue ring. A menu opens on its first item - **Up** / **Down** move, **Esc** closes it. Settings, a dialog or
a menu keeps **Tab** inside it while open, and **Esc** takes you back to where you were. In the projects list, **Up**
/ **Down** choose a project and **Enter** opens it. In the terminal, **Tab** goes to the shell; **Ctrl+J** leaves it.

**On macOS** every shortcut in this guide takes **⌘** where it says **Ctrl** (**⌘B**, **⇧⌘P**, **⌘,**), except
**Ctrl+Tab** and **Ctrl+\`**, which stay **⌃** because ⌘Tab and ⌘\` are macOS's own. The find bar's switches are
**⌥⌘C**, **⌥⌘W** and **⌥⌘R**. DNN Manager has the usual menu bar there: **⌘Q** quits, **⌘H** hides it, **⌘M**
minimizes the window, and **Edit** has Undo, Cut, Copy, Paste and Select All for the text fields. Menus, tooltips and
**Settings → Keyboard shortcuts** write the keys as macOS does (⇧⌘P).

**Text fields** edit as the system's own do:

| | Windows and Linux | macOS |
|---|---|---|
| Copy, cut, paste, select all | Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A (also Ctrl+Insert, Shift+Delete, Shift+Insert) | ⌘C, ⌘X, ⌘V, ⌘A |
| Undo, redo | Ctrl+Z; Ctrl+Y or Ctrl+Shift+Z | ⌘Z; ⇧⌘Z |
| A word left / right (Shift: select it) | Ctrl+← / Ctrl+→ | ⌥← / ⌥→ |
| The start / end (Shift: select to it) | Home / End | ⌘← / ⌘→, Home / End, ⌃A / ⌃E |
| Delete a word / to the start | Ctrl+Backspace, Ctrl+Delete | ⌥⌫, ⌥⌦; ⌘⌫ |

With the mouse: drag to select (also past the field's edge), double-click selects a word, triple-click the whole
line; a right-click has Undo, Redo, Cut, Copy, Paste and Select all. A text longer than the field scrolls to keep the
cursor in sight. In the terminal, **Ctrl+C** copies a selection (without one it stops the program) and **Ctrl+V**
pastes; on macOS **⌘C** copies, **⌘V** pastes and **⌃C** always stops the program.

## Projects

One row per website: the dot is green when it runs, an empty circle when stopped, orange while it changes. Click a
name for its **details**; right-click a row (or **⋮** in the details) for everything you can do with it:

| Action | What happens |
|---|---|
| Start, Stop, Restart (**F5**, **Shift+F5**, **Ctrl+Shift+R**) | The site and its app pool in IIS |
| Keep warm (the flame) | DNN Manager visits the site every few minutes so it answers at once |
| Open with ▸ | The site's folder in an editor, or its database in SQL Server Management Studio |
| Clear website cache… | Empties DNN's cache folders (`Portals\_default\Cache`, `App_Data\ClientDependency`) and recycles the app pool; the site builds them again on its next visit |
| Rename… | The folder, the IIS site and pool, the host name and the database together |
| Clone… | A copy with its own files, database and IIS site |
| Upgrade DNN… | Through every version on DNN's upgrade path, with a backup before each step |
| Export ▸ | **For deployment…** (a package for a server) and backups of the site, the database or both |
| Restore backup ▸ | Puts the site and its database back from a backup |
| Remove… (**Del**) | The IIS site and app pool; its database when it is on this PC and no other site uses it; its folder when it is in the projects folder; its backups only if you choose. It asks first and can't be taken back. |

Every action that changes something runs as one **operation**: its steps show in **Output**, **Cancel** stops it, and
when a step fails or you cancel, what was already done is undone (Remove, which can't be, asks first). Only one
operation runs at a time.

The details' tabs - **General**, **IIS**, **DNN**, **Database**, **Advanced** - show what DNN Manager reads from IIS,
the site's folder, `web.config` and its database, with a health check. Their buttons edit host names, the app pool,
the database connection and the DNN host password. In **Edit host names**, a host name typed in the empty row at the
bottom is saved at once with **Add** (or Enter): the IIS binding, DNN's portal alias and the hosts file are updated
straight away; renamed and removed host names are saved with **Save**.

## New project

Creates a website from a fresh DNN release (downloaded and installed for you), or imports one from a `.zip` of its
files and its `.bacpac` / `.bak`. Fill in the name, the host name and port, the DNN version, the administrator account
and the database; **Still needed** lists what is missing. What you type is kept until the project is created.
Every password box has an eye button that shows and hides what you typed.

**Portal aliases.** An imported site keeps its live addresses and gets local ones beside them, so it opens on this
PC and goes back to the server as it is:

- every portal gets a local address, made primary - portal 0 the site's own (`goededoelen.dnndev.me`), every other
  portal one under it, named after its live domain (`projectflip.goededoelen.dnndev.me` for `projectflip.bondtest.nl`;
  `portal3.goededoelen.dnndev.me` for a portal without one) - and the IIS site answers at all of them (the hosts file
  follows);
- the live aliases stay as the portal's other aliases, and the one that was primary is remembered in the database;
- **Clone** and **Rename** move the local addresses to the new name; child-portal aliases with a path
  (`site/child`) are left as they are;
- **Export → For deployment** leaves every portal's local aliases out of the package and makes the remembered live
  alias primary again; live domains you type there replace portal 0's. A portal made on this PC has no live alias -
  the Output says so; add its domain in DNN once it runs on the server.

While a project is being made (or any other operation runs), the page is dimmed and can't be used - a note above it
says what runs, and it opens again when that is done. On macOS and Linux it stays locked: it needs IIS.

## Source Control

**Source Control** (**Ctrl+3**, ⌘3 on macOS) compares a project's files with a server's - over SFTP, FTPS or FTP - and
sends them either way, as VS Code's Source Control lists changes. Nothing on a server is replaced or deleted without
asking you first. As in VS Code, the **primary side bar** holds the view - the title with **Refresh**, **Upload**,
**Download** and **…**, the project and server, one main button (**Upload**, else **Download**, else **Refresh** -
**Add server** while there is none), the status, the filter box, and the files - and the comparison of the file you choose fills the main view. Drag the side bar's edge to widen it; **Ctrl+B** hides
it.

1. **+** adds a **server**, laid out as FileZilla's Site Manager: its **Name**; the **Protocol** (FTP or SFTP), the
   **Host** and **Port**; for FTP the **Encryption** (**Require explicit FTP over TLS**, or **Only use plain FTP
   (insecure)**); the **Logon Type** (**Normal**: a password - or over SFTP **Key file**), the **User** and the
   **Password**; and the **Default remote directory**, `/site/wwwroot` to start with (Azure App Service's). Optional:
   **Mappings** to deploy only parts of the project, each to its own server folder
   (`DesktopModules/Shop => /site/wwwroot/DesktopModules/Shop`, `;` between them); more files to ignore; files to
   protect; and **Back up the server's copy** before replacing or deleting it. The password goes in your account's
   secure store, never in the settings. A server isn't tied to a project: choose any project and any server, and
   each project keeps its own comparison, records and history with it.
2. **Refresh** (↻) reads both sides and compares them. The first time, for SFTP DNN Manager shows the server's key
   fingerprint and asks whether to trust it; later a different key is refused (edit and save the server to
   trust a new one). FTPS checks the server's certificate against the ones your computer trusts, every time - a
   self-signed certificate is refused. Plain FTP sends the password and the files unencrypted: DNN Manager warns
   once and connects only if you say so - use SFTP or FTPS when the server has either. Generated and local-only files are never compared: `.git/`, `node_modules/`, `App_Data/`, logs,
   caches, `*.tmp`, `*.log`, `*.bak`.
3. The files are listed in **Conflicts**, **Changes** and **Server changes**, each with its count, as VS Code lists
   them: one row a file - its name in its change's colour, its folder beside it, its letter on the right; under the
   mouse the row's own buttons (revert to the server's copy, upload, download, or for a conflict keep mine / keep the
   server's), and on a section's row upload or download all of it. **…** has **View as Tree** (folders) and **Show
   unchanged files**:

| | Means | What to do |
|---|---|---|
| **M** Modified, **A** Added | Changed or new here since the last sync | **Upload** |
| **I** Changed on the server, **R** Only on the server | Changed there, or only there | **Download** - a file only on the server is never deleted because it is missing here |
| **C** Conflict | Changed on both sides | Compare, then **Keep mine** (upload) or **Keep the server's** (download) |
| **D** Deleted | Deleted here after a sync; the server still has it | **Delete on server…** only if you mean to |
| **S** Deleted on the server | Deleted there after a sync | **Upload** to put it back, or leave it |

Click a file to see the server's copy (left) next to the project's (right), the changed lines coloured and the code
in its syntax colours (C#, JavaScript, TypeScript, JSON, CSS, HTML, XML, ASP.NET and Razor files, config, SQL);
**Inline** shows the changes in one column instead. ↑ ↓ (or **F7** / **Shift+F7**) go from change to change. A
binary file shows its sizes and times instead. While the page is open, a file you save, add or delete in the project
updates the list by itself - against the server as the last Refresh read it; **Refresh** to see the server's own
changes.

A **conflict** can also be merged by hand: **Merge…** writes both copies into the project's file, each difference
between `<<<<<<< <environment>`, `=======` and `>>>>>>> project` (git's markers), keeps your copy in the backups
folder first, and opens the file in your editor (VS Code when it is installed). Keep what belongs, remove the
markers, save, **Refresh**, then **Keep mine** to upload the merged file - an upload is refused while markers are
left. Ctrl+click (⌘ on
macOS) and Shift+click select several; the right-click menu has Compare, Upload, Download, Revert local, the conflict
choices and Delete on server. **Upload** and **Download** in the title and the main button take the selected files, or
all those shown; the filter box narrows the list to the files whose path holds what you type.

Before anything is sent, a **preview** lists every file and what happens to it - new on the server, replaced there,
deleted there, new in the project, replaced in the project - with the sizes, the protected files marked, and what
is left alone and why (a conflict without a choice, a file no mapping holds). **Deploy** sends them; Cancel or Esc
sends nothing. When the page opens on a server it shows the last comparison at once - the server as it was
last read, the project as it is now - marked "saved"; **Refresh** reads the server again. **⋮** has Upload all, Download all, the history of what was sent,
the backups folder and the server's edit and delete.

What keeps a server safe: each file is checked again right before it is sent, and left alone if it changed on
either side since the comparison; replacing or deleting server files is shown in the preview first, naming them;
`web.config`, `*.config`, `bin/`, `App_Data/` and `Portals/` are asked about again; with backups on, the server's
copy is kept first in `backups\<project>\server\`. An upload is written under a temporary name and put in place
only once it is whole, so one that breaks off leaves the old file; a lost connection is tried again, up to three
times. Sending shows in **Output** with each file, and **Cancel** stops it between files.

## Settings and Troubleshoot

**Settings** (**Ctrl+,**) has General, Projects, DNN releases, Database server, Docker container, IIS, Keyboard
shortcuts, Help and About. Changes apply when you press **Save**. **Ctrl+F** there goes to its search.

**Keyboard shortcuts**: a command can have several shortcuts. Click one to record a new one in its place (Backspace
removes it, Esc cancels), **+** to add another; the arrow puts the command's default back. The panel and side bar
commands are toggles - **Toggle panel visibility** (Ctrl+J), **Toggle terminal** (Ctrl+\`), **Toggle Output**
(Ctrl+Shift+U): run on the tab already shown, they hide the panel.

**Troubleshoot** (gear menu): **Restart** DNN Manager; **Clean up data** (logs, kept DNN packages, backups, deployment
packages, with their sizes); **Reset settings**; **Reset to factory defaults** (everything but your projects and their
backups).

## Where DNN Manager keeps its files

`Documents\DnnManager` (on macOS and Linux: `~/Documents/DnnManager`):

| | |
|---|---|
| `dnnmanager.db` | Settings, the window's layout, keep warm, what you typed in forms |
| `logs\` | DNN Manager's own log, one file per day - also in **Logs → DNN Manager** |
| `backups\<project>\` | Backups made by Export, Upgrade and Restore |
| `deployments\` | Packages made by Export for deployment |
| `backups\<project>\server\` | Source Control's copies of server files, kept before they were replaced or deleted |

Passwords are never kept in plain text. The default host password and the database server login's password are in
your account's secure store: the Windows Credential Manager, the macOS Keychain, or on Linux the keyring (GNOME
Keyring or KWallet - one must be running). The SA password is in the settings, encrypted with AES-256-GCM; its key is
in that same store, so another user or another computer can't decrypt it. On macOS the Keychain may ask once to let
DNN Manager use it - choose **Always Allow**. An SA password saved by an earlier Windows version (DPAPI) still opens
and is saved in the new form the next time you press **Save**.

## On macOS and Linux

IIS exists only on Windows, so there are no IIS websites. Instead **Projects** lists the DNN folders in your
projects folder (**Settings → Projects**) - every folder with `bin/DotNetNuke.dll` - with their DNN version and
database, as **Folder - no IIS**: there is nothing to start or stop, and the actions that need IIS, Windows or its
tools aren't offered or say so. The window, Settings, the release lists, the status bar's memory, processor and
disk, and checking a SQL Server connection with a SQL login work. **New project** is shown but locked. The projects
folder is `DNN` in your home folder unless you choose another (`C:\DNN` on Windows); the disk shown is its disk.
Shortcuts take ⌘ there ([the window](#the-window)).

## When something goes wrong

- **"Reconnecting…" on Projects**: IIS can't be read - usually DNN Manager runs without administrator rights. Restart
  it and accept the UAC prompt.
- **An operation failed**: **Show output** on its message (or **Ctrl+Shift+U**) shows each step and the error; what it
  had done was undone. DNN Manager's own log is under **Logs → DNN Manager**.
- **Something stays wrong**: Troubleshoot → **Restart**, then **Reset settings**.
- Report problems on [GitHub](https://github.com/Albadit/DnnManager.NET/issues) with the Output's text and the day's
  log file.
