//! The getting started guide - `Controls/GuideDialog.xaml(.cs)` with its texts from `Services/Onboarding.cs` - and,
//! with fewer pages, a page's help (F1) or what a new version added: one page at a time, the pages listed on the left,
//! Back and Next at the bottom (Enter is Next), Skip guide or Esc to leave at any time. It emits a [`GuideResult`].
//!
//! The texts are written for someone who has never used DNN Manager, IIS or DNN; change them with the UI they
//! describe. [`onboarding`] keeps which guide the user saw (the `onboarding` state area, as the WPF app keeps it).

use dnnm_ui::px;
use dnnm_ui::{Icon, IconSize};
use gpui::{App, Bounds, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, PathBuilder, Pixels, ScrollHandle, SharedString, Window, actions, canvas, div, point, prelude::*};

use dnnm_ui::icons::{self, LayoutIcon};
use dnnm_ui::theme::Theme;
use dnnm_ui::{ButtonKind, Pad, Tip, button, icon, icon_sized, link, status_dot};

use crate::help_menu::HelpPage;
use crate::markdown::bold_text;

// ─── The texts ──────────────────────────────────────────────────────────────

/// How much an action can change: shown as a badge next to it (`GuideRisk`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideRisk {
    None,
    /// Changes nothing, or only adds something.
    Safe,
    /// Interrupts a website for a moment, or changes it in a way that can be changed back.
    Careful,
    /// Deletes or replaces something - it can't be undone.
    Destructive,
}

/// One line of a guide page: an icon, a name and what it means or does. `icon` is an icon key of `Themes/Icons.xaml`
/// (`GlyphPlay`, `FlameFilled`, `LayoutPanel`), a number (a step in a list) or `Dot.Running` / `Dot.Stopped` /
/// `Dot.Busy` / `Dot.None` (the Projects table's status dot). `**text**` in `text` is bold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideItem {
    pub icon: &'static str,
    pub name: &'static str,
    pub text: &'static str,
    pub risk: GuideRisk,
}

/// A heading on a guide page and its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideSection {
    pub heading: &'static str,
    pub items: &'static [GuideItem],
}

/// One page of the guide. `since` is the guide [`VERSION`] that added it: after an update, a user who went through an
/// older guide is shown only the pages added since (see [`at_start`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideStep {
    pub title: &'static str,
    /// The short name in the guide's step list.
    pub short_title: &'static str,
    pub intro: &'static str,
    pub sections: &'static [GuideSection],
    pub tip: Option<&'static str>,
    pub since: i32,
}

const fn item(icon: &'static str, name: &'static str, text: &'static str) -> GuideItem {
    GuideItem { icon, name, text, risk: GuideRisk::None }
}

const fn risk(icon: &'static str, name: &'static str, text: &'static str, risk: GuideRisk) -> GuideItem {
    GuideItem { icon, name, text, risk }
}

const fn section(heading: &'static str, items: &'static [GuideItem]) -> GuideSection {
    GuideSection { heading, items }
}

use GuideRisk::{Careful, Destructive, Safe};

/// The guide's version. Raise it when a release adds a page users should see (give the page `since` = the new version):
/// those who saw the older guide get only the new pages at their next start - not the whole guide again.
pub const VERSION: i32 = 1;

/// What the start shows of the guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideAtStart {
    Nothing,
    Welcome,
    NewSteps,
}

/// What to show at this start: the whole guide on DNN Manager's very first start; the pages added since the guide the
/// user last saw; or nothing. Someone who used DNN Manager before the guide existed (`seen_version` 0 but not a first
/// start) isn't shown it - they know the app; Help opens it.
pub fn at_start(
    seen_version: i32,
    first_start: bool,
    guide: &'static [GuideStep],
    version: i32,
) -> (GuideAtStart, Vec<&'static GuideStep>) {
    if seen_version >= version {
        return (GuideAtStart::Nothing, Vec::new());
    }
    if seen_version <= 0 {
        return if first_start {
            (GuideAtStart::Welcome, guide.iter().collect())
        } else {
            (GuideAtStart::Nothing, Vec::new())
        };
    }
    let added: Vec<&GuideStep> = guide.iter().filter(|s| s.since > seen_version).collect();
    if added.is_empty() { (GuideAtStart::Nothing, added) } else { (GuideAtStart::NewSteps, added) }
}

/// The help for a page (the ? button, F1) - the Projects page's for anything else (`Onboarding.HelpFor`).
pub fn help_for(page: HelpPage) -> Vec<&'static GuideStep> {
    match page {
        HelpPage::Details => vec![&DETAILS],
        HelpPage::Setup => vec![&NEW_PROJECT],
        HelpPage::Settings => vec![&SETTINGS],
        HelpPage::Troubleshoot => vec![&TROUBLESHOOTING],
        HelpPage::Projects => vec![&WEBSITES, &ACTIONS],
    }
}

/// The whole guide, in order.
pub static GUIDE: [GuideStep; 8] =
    [WELCOME, BEFORE_YOU_START, NAVIGATION, WEBSITES, ACTIONS, TROUBLESHOOTING, UPDATES, READY];

const WELCOME: GuideStep = GuideStep {
    title: "Welcome to DNN Manager",
    short_title: "Welcome",
    intro: "DNN Manager runs DNN websites on this computer, so you can build and try them out before they go live. \
            DNN (also called DotNetNuke) is a system for building websites.",
    sections: &[
        section(
            "With DNN Manager you can",
            &[
                item(
                    "GlyphAdd",
                    "Create a website",
                    "Make a new DNN website in a few clicks. DNN Manager downloads and installs everything for you.",
                ),
                item(
                    "GlyphFolderOpen",
                    "Bring in an existing website",
                    "Put a copy of a website on this computer from a backup of its files and its database.",
                ),
                item(
                    "GlyphPlay",
                    "Start and stop websites",
                    "Turn websites on and off, and see at a glance which ones are running.",
                ),
                item(
                    "GlyphCopy",
                    "Back up, copy and upgrade",
                    "Make a backup, put a backup back, copy a website, or move it to a newer DNN version.",
                ),
                item(
                    "GlyphInfo",
                    "Find out what is wrong",
                    "When a website doesn't work, DNN Manager shows what happened and where to look.",
                ),
            ],
        ),
        section(
            "Words you will see",
            &[
                item("GlyphList", "Project", "One website on this computer, with its files and its database."),
                item(
                    "GlyphGlobe",
                    "IIS",
                    "The web server that comes with Windows. It is the program that shows your websites in the browser.",
                ),
                item("GlyphDatabase", "Database", "Where a website keeps its pages, users and content."),
                item(
                    "GlyphDatabase",
                    "Database server",
                    "The program that looks after the databases. It has to be running for your websites to work.",
                ),
                item(
                    "GlyphContainer",
                    "Docker",
                    "A free program that can run a database server for you. DNN Manager can install it and set it up.",
                ),
            ],
        ),
    ],
    tip: Some(
        "This guide takes about three minutes. **Skip guide** leaves it at any time - open it again later with the **?** at the top right.",
    ),
    since: 1,
};

const BEFORE_YOU_START: GuideStep = GuideStep {
    title: "Before you start",
    short_title: "Before you start",
    intro: "DNN Manager needs a few things on this computer before it can create websites. You only do this once.",
    sections: &[
        section(
            "Needed",
            &[
                item(
                    "GlyphWarning",
                    "Permission to make changes",
                    "Every time DNN Manager starts, Windows asks whether it may make changes to this computer. Click **Yes** - DNN Manager needs it to manage websites.",
                ),
                item(
                    "GlyphGlobe",
                    "IIS, the web server",
                    "Open **Settings → IIS** and click **Test**. If something is missing, **Set up IIS** turns it on. Windows may ask you to restart the computer.",
                ),
                item(
                    "GlyphDatabase",
                    "A database server",
                    "**Settings → Database server** chooses where websites keep their data. The standard choice, the local SQL container, uses Docker: \
                 open **Settings → Docker container**, click **Test** and use the button it offers - **Install Docker Desktop**, **Start Docker Desktop**, then **Set up docker-compose**.",
                ),
                item(
                    "GlyphDownload",
                    "An internet connection",
                    "Needed to download DNN when you create a new website. Versions downloaded before can be kept for use without internet (**Settings → DNN releases**).",
                ),
            ],
        ),
        section(
            "Optional - change these any time",
            &[
                item(
                    "GlyphFolder",
                    "Projects folder",
                    "Where the files of new websites are stored - C:\\DNN unless you choose another. **Settings → Projects**.",
                ),
                item(
                    "GlyphKeyboard",
                    "Login of new websites",
                    "The user name and password every new website gets for its main administrator (the **host** account). Choose your own password in **Settings → Projects → DNN defaults**.",
                ),
                item(
                    "GlyphSettings",
                    "Start with Windows",
                    "**Settings → General → Start DNN Manager when you sign in to your computer**.",
                ),
                item("GlyphShow", "Light or dark", "The gear at the bottom left → **Themes**."),
            ],
        ),
    ],
    tip: Some(
        "Settings keeps your changes only after you click **Save**. The **Test** and **Set up** buttons use the saved settings - click **Save** first.",
    ),
    since: 1,
};

const NAVIGATION: GuideStep = GuideStep {
    title: "Finding your way around",
    short_title: "Finding your way",
    intro: "The window has the same parts as many modern apps. This is what each part is for.",
    sections: &[
        section(
            "The pages on the left",
            &[
                item(
                    "GlyphList",
                    "Projects",
                    "Every website on this computer. See which ones are running, and do things with them.",
                ),
                item(
                    "GlyphAdd",
                    "New project",
                    "Create a new website - or make one from a backup (a .zip of its files with its database file).",
                ),
                item(
                    "GlyphSettings",
                    "The gear",
                    "A menu with **Settings**, **Keyboard Shortcuts**, **Themes**, **Customize Layout**, **Troubleshoot** and **Check for Updates**.",
                ),
            ],
        ),
        section(
            "Along the top",
            &[
                item(
                    "GlyphSearch",
                    "Search",
                    "Find a website by typing part of its name. Type **>** first to find any command instead.",
                ),
                item("GlyphQuestion", "Help", "Help for the page you are on, this guide, and a tour of the window."),
                item("GlyphDownload", "Update", "Shows only when a new version of DNN Manager is ready to install."),
                item(
                    "LayoutPanel",
                    "Layout buttons",
                    "Show or hide the pages on the left and the panel at the bottom.",
                ),
            ],
        ),
        section(
            "Along the bottom",
            &[
                item(
                    "Dot.Running",
                    "IIS running",
                    "Whether the web server is on. Green means your websites can be opened.",
                ),
                item(
                    "GlyphInfo",
                    "RAM, CPU, Disk",
                    "How busy this computer is: memory, processor, and how full the disk is.",
                ),
                item(
                    "LayoutPanel",
                    "The panel",
                    "**Output**, **Logs** and **Terminal**. Open it with the panel button at the top right, or **Ctrl+J**.",
                ),
                item(
                    "GlyphCancel",
                    "Bottom right corner",
                    "Shows the task DNN Manager is working on, with **Cancel**, and short messages.",
                ),
            ],
        ),
    ],
    tip: None,
    since: 1,
};

const WEBSITES: GuideStep = GuideStep {
    title: "Your websites",
    short_title: "Your websites",
    intro: "The **Projects** page lists every DNN website on this computer, one per row. The list keeps itself up to date - there is nothing to refresh.",
    sections: &[
        section(
            "The dot shows how a website is doing",
            &[
                item("Dot.Running", "Running", "The website is on and can be opened in the browser."),
                item(
                    "Dot.Stopped",
                    "Stopped",
                    "The website is off. **Start** turns it on. **App pool stopped** means its background process is off - **Start** fixes that too.",
                ),
                item("Dot.Busy", "Busy", "Starting, stopping, or a task is running on it. Wait a moment."),
                item("Dot.None", "No website", "The files are there, but IIS has no website for them."),
            ],
        ),
        section(
            "What the other columns tell you",
            &[
                item(
                    "GlyphDatabase",
                    "SQL",
                    "**Live** (green): the website's database answers. **Offline** (red): the database server can't be reached - check that it is running. **File**: the database is a file in the website's folder.",
                ),
                item(
                    "GlyphWarning",
                    "Database: not set",
                    "The website doesn't say which database to use. Point at it with the mouse to see why.",
                ),
                item(
                    "GlyphInfo",
                    "DNN, CPU, Memory…",
                    "The DNN version, how much the website uses of this computer, and when it last started. **Columns** chooses what you see.",
                ),
                item(
                    "GlyphRefresh",
                    "Reconnecting…",
                    "Shows at the top right when DNN Manager can't read the websites for a moment. It sorts itself out.",
                ),
            ],
        ),
        section(
            "What a click does",
            &[
                item(
                    "GlyphList",
                    "The name",
                    "Opens the website's details: everything about it on five tabs, with buttons to change it.",
                ),
                item("GlyphGlobe", "The address", "Opens the website in your browser."),
                item("GlyphChevronRight", "The arrow", "Shows a few facts under the row."),
                item("GlyphMore", "Right-click a row", "Every action for that website, in one menu."),
                item(
                    "GlyphSearch",
                    "Search, Columns, Only show running",
                    "Find a website, choose the columns, or hide the websites that are stopped.",
                ),
            ],
        ),
    ],
    tip: None,
    since: 1,
};

const ACTIONS: GuideStep = GuideStep {
    title: "Buttons and what they do",
    short_title: "Buttons and actions",
    intro: "Each row has buttons on the right. Point at any button with the mouse to see what it does. Whatever deletes something asks first.",
    sections: &[
        section(
            "In each row",
            &[
                risk("GlyphPlay", "Start", "Turns the website on so it can be opened in the browser.", Safe),
                risk(
                    "GlyphStop",
                    "Stop",
                    "Turns the website off. Nobody can open it until you start it again. Nothing is deleted.",
                    Careful,
                ),
                risk(
                    "GlyphRefresh",
                    "Restart",
                    "Restarts the website's background process. This can fix some problems, but the website is unavailable for a few seconds.",
                    Careful,
                ),
                risk(
                    "FlameFilled",
                    "Keep warm",
                    "Keeps the website ready, so it opens at once even after a while without visits - otherwise the first page can take up to 10 seconds. \
                 Works while DNN Manager runs. Orange: on. Outline: off. Grey: paused. A red dot: it is having trouble.",
                    Safe,
                ),
                risk(
                    "GlyphDelete",
                    "Remove",
                    "Deletes the website from this computer: its website in IIS, its folder and its database. Its backups are kept. Asks first, naming what goes.",
                    Destructive,
                ),
            ],
        ),
        section(
            "Right-click a row (or ⋮ on the details)",
            &[
                risk(
                    "GlyphGlobe",
                    "Open site / Open folder",
                    "The website in your browser, or its files in File Explorer.",
                    Safe,
                ),
                risk(
                    "GlyphFolderOpen",
                    "Open with…",
                    "The files in a code editor, or the database in SQL Server Management Studio - whichever are installed.",
                    Safe,
                ),
                risk(
                    "GlyphList",
                    "View logs",
                    "What the website wrote down - the place to look when it shows an error.",
                    Safe,
                ),
                risk(
                    "GlyphReset",
                    "Clear website cache…",
                    "Deletes the website's temporary files and restarts it. Helps when pages show old content. The next page loads a little slower.",
                    Careful,
                ),
                risk(
                    "GlyphKeyboard",
                    "Rename…",
                    "A new name for the website - its folder and address can change too. It is stopped for a moment.",
                    Careful,
                ),
                risk(
                    "GlyphCopy",
                    "Clone…",
                    "A copy of the website under a new name. The original isn't changed.",
                    Safe,
                ),
                risk(
                    "GlyphDownload",
                    "Upgrade DNN…",
                    "Moves the website to a newer DNN version, one step at a time. A backup is made before each step and put back if a step fails.",
                    Careful,
                ),
                risk(
                    "GlyphCopy",
                    "Export",
                    "A backup of the website (its files, its database or both), or a package to put it on a live server.",
                    Safe,
                ),
                risk(
                    "GlyphReset",
                    "Restore backup",
                    "Puts the website back the way it was in a backup. Changes made since that backup are lost.",
                    Destructive,
                ),
            ],
        ),
        section(
            "Several websites at once",
            &[item(
                "GlyphCheck",
                "Tick the boxes",
                "Tick the boxes in front of websites and a group of buttons appears above the list: **Start**, **Stop**, **Restart**, **Keep warm** and **Remove**. Each works on every ticked website it fits.",
            )],
        ),
        section(
            "While a task runs",
            &[risk(
                "GlyphCancel",
                "Cancel",
                "The bottom right corner shows the task. **Cancel** stops it and undoes what it did so far. One task runs at a time.",
                Safe,
            )],
        ),
        section(
            "IIS, at the bottom left",
            &[
                risk("GlyphPlay", "Start IIS", "Turns the web server on, so your websites can be opened.", Safe),
                risk(
                    "GlyphRefresh",
                    "Restart IIS",
                    "Every website on this computer is unavailable for a few seconds. Fixes some errors - for example after Windows parts were installed.",
                    Careful,
                ),
                risk(
                    "GlyphStop",
                    "Stop IIS",
                    "Every website on this computer stops until you start IIS again.",
                    Careful,
                ),
            ],
        ),
    ],
    tip: None,
    since: 1,
};

const TROUBLESHOOTING: GuideStep = GuideStep {
    title: "When something goes wrong",
    short_title: "Logs and troubleshooting",
    intro: "Logs are like a diary: DNN Manager, your websites, IIS and Windows write down what they do - and what went wrong. \
            You don't need them day to day, only when something doesn't work.",
    sections: &[
        section(
            "The panel at the bottom (Ctrl+J)",
            &[
                item(
                    "GlyphList",
                    "Output",
                    "What DNN Manager is doing, step by step. Green **SUCCESS**: it worked. Yellow **WARN**: worth a look. Red **ERROR**: it failed - the line under it says what to do. \
                 A red dot on **Output** means the last task failed.",
                ),
                item(
                    "GlyphSearch",
                    "Logs",
                    "Choose a website and a log at the top. Its logs say why it shows an error page. DNN Manager's own log is listed first. Errors are red, warnings yellow.",
                ),
                item(
                    "GlyphKeyboard",
                    "Terminal",
                    "A command line, for advanced users. You don't need it to use DNN Manager.",
                ),
            ],
        ),
        section(
            "A website doesn't work? Try these in order",
            &[
                item(
                    "1",
                    "Read the message",
                    "Errors show in the bottom right corner. **Show output** opens the details.",
                ),
                item(
                    "2",
                    "Check IIS",
                    "The bottom left has to say **IIS running**. If it doesn't, click its **Start** button.",
                ),
                item(
                    "3",
                    "Check the database",
                    "When the **SQL** column says **Offline**, the database server isn't running. With Docker: start Docker Desktop, or open **Settings → Docker container** and click **Test**.",
                ),
                item("4", "Restart the website", "The **Restart** button in its row."),
                item("5", "Look at its logs", "Right-click the website → **View logs**."),
            ],
        ),
        section(
            "Troubleshoot (the gear → Troubleshoot)",
            &[
                risk("GlyphRefresh", "Restart", "Closes DNN Manager and opens it again. Nothing is lost.", Safe),
                risk(
                    "GlyphDelete",
                    "Clean up data",
                    "Deletes what you tick, to free disk space: old logs, downloaded DNN files and - only when you tick them - the project backups.",
                    Careful,
                ),
                risk(
                    "GlyphReset",
                    "Reset settings to defaults",
                    "Puts every setting back as it was when DNN Manager was installed, saved passwords too. Your websites aren't touched.",
                    Careful,
                ),
                risk(
                    "GlyphWarning",
                    "Reset to factory defaults",
                    "Removes the settings, saved passwords, logs and downloaded files, and starts DNN Manager as if it was just installed. Your websites and their backups are kept.",
                    Destructive,
                ),
            ],
        ),
    ],
    tip: Some("Troubleshoot never changes your websites - their files and databases stay as they are."),
    since: 1,
};

const UPDATES: GuideStep = GuideStep {
    title: "Updates",
    short_title: "Updates",
    intro: "DNN Manager looks for a new version a few seconds after it starts.",
    sections: &[section(
        "How updating works",
        &[
            item(
                "GlyphDownload",
                "The Update button",
                "A blue **Update** button shows at the top right when a new version is ready. You don't have to install it right away.",
            ),
            item(
                "GlyphRefresh",
                "When you click it",
                "DNN Manager downloads the new version and checks that it is genuine. Then it **closes and opens again by itself**, on the page where you were. It takes about a minute.",
            ),
            item(
                "GlyphCheck",
                "Your work is safe",
                "Your websites, settings and backups aren't touched. When a task is running, DNN Manager asks before it closes. If the update fails, the old version keeps working.",
            ),
            item("GlyphInfo", "What's new", "After an update, a window shows what changed."),
            item("GlyphSearch", "Look yourself", "The gear → **Check for Updates…**, or **Settings → About**."),
        ],
    )],
    tip: None,
    since: 1,
};

const READY: GuideStep = GuideStep {
    title: "You're ready",
    short_title: "Ready",
    intro: "That's it. A good way to begin:",
    sections: &[
        section(
            "Your first website",
            &[
                item(
                    "1",
                    "Check the settings",
                    "**Settings → IIS** and **Settings → Database server**: click **Test**, and set up what is missing.",
                ),
                item(
                    "2",
                    "Create a website",
                    "**New project**: type a name and click **Create project**. The panel at the bottom shows each step.",
                ),
                item("3", "Open it", "On **Projects**, click the website's address to open it in your browser."),
            ],
        ),
        section(
            "Help is always close",
            &[
                item(
                    "GlyphQuestion",
                    "? at the top right",
                    "Help for the page you are on (**F1**), this guide and the tour.",
                ),
                item("GlyphSettings", "Settings → Help", "This guide and the tour, whenever you want them again."),
                item("GlyphInfo", "Point at a button", "Its tooltip says what it does."),
            ],
        ),
    ],
    tip: Some("Want to see where everything is? **Take the tour** points at each part of the window."),
    since: 1,
};

// ─── Help for a page ────────────────────────────────────────────────────────

const DETAILS: GuideStep = GuideStep {
    title: "A website's details",
    short_title: "A website's details",
    intro: "Everything about one website, read from the website itself. **Back** (or **Esc**) returns to the list.",
    sections: &[
        section(
            "At the top",
            &[
                risk("GlyphPlay", "Start, Stop, Restart", "The same as the buttons in its row on Projects.", Careful),
                risk(
                    "GlyphGlobe",
                    "Open site / Open folder",
                    "The website in your browser, or its files in File Explorer.",
                    Safe,
                ),
                item(
                    "GlyphMore",
                    "⋮ More actions",
                    "Logs, clearing the cache, export, backups and more - the same as right-clicking the website.",
                ),
            ],
        ),
        section(
            "The tabs",
            &[
                item(
                    "GlyphInfo",
                    "General",
                    "Its folder, DNN version and backups, and a health check listing anything that is wrong.",
                ),
                item(
                    "GlyphGlobe",
                    "IIS",
                    "How the web server runs it: its addresses, its background process, and **Keep warm**.",
                ),
                item(
                    "GlyphList",
                    "DNN",
                    "The DNN installation, its portals (the sites inside it), extensions and administrator (host) accounts.",
                ),
                item("GlyphDatabase", "Database", "Where its data is, and whether the database answers."),
                item(
                    "GlyphSettings",
                    "Advanced",
                    "Technical settings, limits, file permissions and every problem found.",
                ),
            ],
        ),
        section(
            "Buttons that change the website",
            &[
                risk("GlyphKeyboard", "Rename…", "A new name - its folder and address can change too.", Careful),
                risk("GlyphGlobe", "Edit host names…", "The addresses the website answers on.", Careful),
                risk(
                    "GlyphSettings",
                    "Edit app pool…",
                    "How its background process runs. For advanced users.",
                    Careful,
                ),
                risk(
                    "GlyphDatabase",
                    "Change connection…",
                    "Points the website at another database. Nothing is copied or moved.",
                    Careful,
                ),
                risk(
                    "GlyphKeyboard",
                    "Change host password…",
                    "A new password for an administrator account. The website restarts.",
                    Careful,
                ),
                risk("GlyphDownload", "Upgrade DNN…", "A newer DNN version, with a backup before each step.", Careful),
            ],
        ),
    ],
    tip: Some("An orange dot marks a value worth a look, a red dot a problem - the line under it says why."),
    since: 1,
};

const NEW_PROJECT: GuideStep = GuideStep {
    title: "New project",
    short_title: "New project",
    intro: "Creates a website on this computer. Fill in the form from top to bottom - **Create project** can be clicked once everything is filled in correctly.",
    sections: &[
        section(
            "The form",
            &[
                item(
                    "1",
                    "Project name",
                    "Checked as you type. It becomes the name of the website's folder and part of its address.",
                ),
                item(
                    "2",
                    "Host name and Port",
                    "The website's address, for example mysite.dnndev.me. The suggestion is fine.",
                ),
                item(
                    "3",
                    "Start from",
                    "**A new site**: DNN Manager downloads DNN and installs it. **An existing site**: a .zip of the website's files with its database backup (.bacpac or .bak).",
                ),
                item(
                    "4",
                    "Repository and Version",
                    "Where DNN comes from, and which version. The newest is already chosen - keep it if you're not sure.",
                ),
                item(
                    "5",
                    "DNN installation",
                    "**Automatic setup** (recommended): the website is ready on your first visit. **Manual**: DNN asks you to fill in its own setup pages.",
                ),
                item(
                    "6",
                    "DNN account and website",
                    "The login of the website's main administrator (the host account). Write the password down - **Generate** makes a strong one.",
                ),
                item(
                    "7",
                    "Database",
                    "Where the website keeps its data, filled in from **Settings → Database server**. It is tested before anything is created.",
                ),
            ],
        ),
        section(
            "Then",
            &[risk(
                "GlyphPlay",
                "Create project",
                "Starts. The panel at the bottom shows every step; **Cancel** undoes everything it did.",
                Safe,
            )],
        ),
    ],
    tip: Some("A new site needs an internet connection to download DNN."),
    since: 1,
};

const SETTINGS: GuideStep = GuideStep {
    title: "Settings",
    short_title: "Settings",
    intro: "Pick a category on the left, or type in the search box. Changes are kept only after **Save**; **Discard changes** undoes them. **Close** or **Esc** returns to the page.",
    sections: &[section(
        "The categories",
        &[
            item(
                "GlyphSettings",
                "General",
                "Start with Windows, keep running when the window is closed, the size of everything, and the terminal.",
            ),
            item(
                "GlyphFolder",
                "Projects",
                "Where websites are stored, their address, the login new websites get, and **Keep warm**.",
            ),
            item(
                "GlyphDownload",
                "DNN releases",
                "Where DNN versions come from, keeping downloads for use without internet, and DNN's upgrade path.",
            ),
            item(
                "GlyphDatabase",
                "Database server",
                "Where new websites keep their data: Docker, a SQL Server, or a file.",
            ),
            item(
                "GlyphContainer",
                "Docker container",
                "The database server in Docker. **Test** checks it; its buttons install, start and set it up.",
            ),
            item(
                "GlyphGlobe",
                "IIS",
                "The parts of Windows the web server needs. **Test** checks them; **Set up IIS** turns on what is missing.",
            ),
            item("GlyphKeyboard", "Keyboard shortcuts", "Every keyboard shortcut - change any of them. Saved at once."),
            item("GlyphQuestion", "Help", "The getting started guide, the tour, and the full user guide."),
            item("GlyphInfo", "About", "The version, whether an update is ready, and the folders with your files."),
        ],
    )],
    tip: Some("The **Test** and **Set up** buttons use the saved settings - click **Save** first."),
    since: 1,
};

// ─── The onboarding state ───────────────────────────────────────────────────

/// Which guide the user saw - the `onboarding` area of the `state` table, with the WPF app's keys (`OnboardingState`):
/// `guideVersion`, `skipped`, `firstVersion`.
pub mod onboarding {
    use dnnm_infrastructure::app_db::AppDatabase;

    use super::{GUIDE, GuideAtStart, GuideStep, VERSION, at_start};

    const AREA: &str = "onboarding";

    /// What is kept between starts.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct OnboardingState {
        /// The version of the guide the user went through or skipped ([`VERSION`]) - 0 until then.
        pub guide_version: i32,
        /// Left with Skip guide (or closed) rather than finished.
        pub skipped: bool,
        /// The version DNN Manager's very first start was - `None` for someone who used it before this was kept.
        pub first_version: Option<String>,
    }

    /// The state - its defaults where nothing (usable) is saved, as `StateStore.Load` reads it.
    pub fn load(db: &AppDatabase) -> OnboardingState {
        let get = |key: &str| db.state(AREA, key).ok().flatten();
        OnboardingState {
            guide_version: get("guideVersion").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            skipped: get("skipped").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
            first_version: get("firstVersion"),
        }
    }

    /// Writes `state` - a missing first version has no row, as .NET writes a null.
    pub fn save(db: &AppDatabase, state: &OnboardingState) -> Result<(), String> {
        db.set_state(AREA, "guideVersion", &state.guide_version.to_string())?;
        db.set_state(AREA, "skipped", if state.skipped { "true" } else { "false" })?;
        match &state.first_version {
            Some(v) => db.set_state(AREA, "firstVersion", v),
            None => db.delete_state(AREA, "firstVersion"),
        }
    }

    /// The guide was left - remembered so it isn't shown again (`SaveGuideSeen`): `skipped` when its result was
    /// [`GuideResult::Skipped`](super::GuideResult::Skipped).
    pub fn save_guide_seen(db: &AppDatabase, skipped: bool) -> Result<(), String> {
        let mut state = load(db);
        state.guide_version = VERSION;
        state.skipped = skipped;
        save(db, &state)
    }

    /// At the start, once the workspace is back (`ShowGuideAtStart`): the guide on the very first start, the pages a
    /// newer guide added, or nothing. Nothing to show is remembered here; for the others the app opens
    /// [`GuideDialog::welcome`](super::GuideDialog::welcome) or [`GuideDialog::new_steps`](super::GuideDialog::new_steps)
    /// and calls [`save_guide_seen`] with its result (and starts the tour on `GuideResult::Tour`).
    pub fn guide_at_start(db: &AppDatabase, first_start: bool) -> (GuideAtStart, Vec<&'static GuideStep>) {
        let state = load(db);
        if state.guide_version >= VERSION {
            return (GuideAtStart::Nothing, Vec::new());
        }
        let (what, steps) = at_start(state.guide_version, first_start, &GUIDE, VERSION);
        if what == GuideAtStart::Nothing {
            let _ = save_guide_seen(db, state.skipped);
        }
        (what, steps)
    }

    /// Notes the version of the very first start and says whether the user is still on it (`NoteFirstVersion`,
    /// `IsNewUser`): What's new isn't offered - not at the start, in Help, Settings → Help or the command palette -
    /// until the first update.
    pub fn note_first_version(db: &AppDatabase, first_start: bool, current: &str) -> bool {
        let mut state = load(db);
        if first_start && state.first_version.is_none() {
            state.first_version = Some(current.to_string());
            let _ = save(db, &state);
        }
        is_new_user(&state, current)
    }

    /// Still on the version DNN Manager first started as.
    pub fn is_new_user(state: &OnboardingState, current: &str) -> bool {
        state.first_version.as_deref() == Some(current)
    }
}

// ─── The dialog ─────────────────────────────────────────────────────────────

/// How the guide was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideResult {
    /// Skip guide, Esc or the window's ✕ - before its last page, or a help page closed.
    Skipped,
    /// Its last page's finish button.
    Finished,
    /// Take the tour, on the last page: the window's tour follows.
    Tour,
}

actions!(guide, [GuideNext, GuideSkip]);

/// Enter is Next (the finish button on the last page), Esc is Skip. Call once at the start.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", GuideNext, Some("GuideDialog")),
        KeyBinding::new("escape", GuideSkip, Some("GuideDialog")),
    ]);
}

/// The window title of the guide (`MainWindow.GuideTitle`).
pub const GUIDE_TITLE: &str = "Getting started";

/// The guide dialog. Emits one [`GuideResult`] when it is left; the app closes it then.
pub struct GuideDialog {
    title: SharedString,
    steps: Vec<&'static GuideStep>,
    skip_text: SharedString,
    finish_text: SharedString,
    offer_tour: bool,
    index: usize,
    seen: Vec<bool>,
    done: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl EventEmitter<GuideResult> for GuideDialog {}

impl GuideDialog {
    /// Shows `steps` (at least one). `offer_tour` adds Take the tour to the last page; `finish_text` is its finish
    /// button; `skip_text` the link that leaves it. Takes the keyboard.
    pub fn new(
        title: impl Into<SharedString>,
        steps: Vec<&'static GuideStep>,
        skip_text: impl Into<SharedString>,
        finish_text: impl Into<SharedString>,
        offer_tour: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut seen = vec![false; steps.len()];
        if let Some(first) = seen.first_mut() {
            *first = true;
        }
        Self {
            title: title.into(),
            steps,
            skip_text: skip_text.into(),
            finish_text: finish_text.into(),
            offer_tour,
            index: 0,
            seen,
            done: false,
            focus,
            scroll: ScrollHandle::new(),
        }
    }

    /// The whole guide: at the very first start, and from Help, Settings → Help or the command palette (`ShowGuide`).
    pub fn welcome(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(GUIDE_TITLE, GUIDE.iter().collect(), "Skip guide", "Start using DNN Manager", true, window, cx)
    }

    /// The pages a newer guide added, after an update.
    pub fn new_steps(steps: Vec<&'static GuideStep>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new("New in DNN Manager", steps, "Close", "Got it", false, window, cx)
    }

    /// What the page shown is for, and what its buttons do (F1, `ShowPageHelp`).
    pub fn page_help(page: HelpPage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(format!("Help: {}", page.name()), help_for(page), "Close", "Done", false, window, cx)
    }

    fn show_step(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.steps.len() || index == self.index {
            return;
        }
        self.index = index;
        self.seen[index] = true;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn finish(&mut self, result: GuideResult, cx: &mut Context<Self>) {
        if !self.done {
            self.done = true;
            cx.emit(result);
        }
    }

    /// Next, or the finish button on the last page.
    pub fn next(&mut self, cx: &mut Context<Self>) {
        if self.index + 1 < self.steps.len() {
            self.show_step(self.index + 1, cx);
        } else {
            self.finish(GuideResult::Finished, cx);
        }
    }

    /// Skip guide, Esc, or the app closing it.
    pub fn skip(&mut self, cx: &mut Context<Self>) {
        self.finish(GuideResult::Skipped, cx);
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.index > 0 {
            self.show_step(self.index - 1, cx);
        }
    }

    /// The step list on the left: a circle with the number (a tick once seen), the short name.
    fn render_steps(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        div()
            .id("guide-steps")
            .flex_none()
            .w(px(230.0))
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(c.sidebar_bg)
            .border_r_1()
            .border_color(c.card_border)
            .gap(px(7.0))
            .p(px(14.0))
            // The title 20 px from the top and 8 px further in than the list's 14; 13 px above the first step.
            .child(
                div()
                    .pad(px(6.0), px(8.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme.text.medium)
                    .child(self.title.clone()),
            )
            .child(div().flex().flex_col().gap(px(2.0)).children(self.steps.iter().enumerate().map(|(i, step)| {
                let current = i == self.index;
                let seen = self.seen[i] && !current;
                let colour = if current || seen { c.accent } else { c.text_muted };
                let mark = if seen {
                    icon_sized(Icon::Check, IconSize::Small, colour, cx).into_any_element()
                } else {
                    div().text_size(theme.text.caption).text_color(colour).child((i + 1).to_string()).into_any_element()
                };
                dnnm_ui::focusable_bare(div().id(("guide-step", i)), cx)
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .pad(px(7.0), px(9.0))
                    .rounded(theme.metrics.control_radius)
                    .cursor_pointer()
                    .when(current, |d| d.bg(c.selection_bg))
                    .when(!current, |d| d.hover(|s| s.bg(c.hover_bg)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show_step(i, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(22.0))
                            .rounded_full()
                            .border(px(1.5))
                            .border_color(colour)
                            .child(mark),
                    )
                    .child(
                        dnnm_ui::one_line(step.short_title),
                    )
            })))
    }

    /// A page: its title, its introduction, each section with its lines, and its tip in the info banner.
    fn render_page(&self, step: &'static GuideStep, cx: &App) -> impl IntoElement {
        let theme = Theme::get(cx);
        let c = theme.colors;
        // 28 px above and below, 34 at the sides (720 wide at most inside them). With one page the title above it
        // brings 16 px of its own, so the page's own is 12 there - 28 between them, as before.
        let vertical = if self.steps.len() > 1 { px(28.0) } else { px(12.0) };
        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .max_w(px(720.0 + 2.0 * 34.0))
            .pad(vertical, px(34.0))
            .line_height(px(20.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w_full()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(theme.text.title)
                            .line_height(px(28.0))
                            .child(bold_text(step.title)),
                    )
                    .child(div().w_full().text_size(theme.text.medium).child(bold_text(step.intro))),
            )
            // A section's heading 7 px above its first item, the items 7 px apart.
            .children(step.sections.iter().map(|section| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.text_muted).child(section.heading))
                    .children(section.items.iter().map(|item| item_row(item, cx)))
            }))
            .when_some(step.tip, |d, tip| {
                d.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.0))
                        .pad(px(14.0), px(18.0))
                        .rounded(theme.metrics.card_radius)
                        .border_1()
                        .border_color(c.info_banner_border)
                        .bg(c.info_banner_bg)
                        .child(dnnm_ui::line_icon(Icon::Info, IconSize::Medium, c.accent, cx))
                        .child(div().flex_1().min_w_0().child(bold_text(tip))),
                )
            })
    }
}

/// An item: its icon in a small rounded box, its name (and how much it can change), what it means or does.
fn item_row(item: &GuideItem, cx: &App) -> impl IntoElement {
    let theme = Theme::get(cx);
    let c = theme.colors;
    div()
        .flex()
        .items_start()
        .child(
            div().flex_none().w(px(42.0)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(30.0))
                    .rounded(px(6.0))
                    .bg(c.hover_bg)
                    .child(item_icon(item.icon, cx)),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(px(8.0))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(item.name))
                        .when_some(badge(item.risk, cx), |d, b| d.child(b)),
                )
                .child(div().w_full().child(bold_text(item.text))),
        )
}

/// How much an action can change, next to its name - nothing for a plain explanation.
fn badge(risk: GuideRisk, cx: &App) -> Option<impl IntoElement> {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let (text, colour) = match risk {
        GuideRisk::Safe => ("Safe", c.success_text),
        GuideRisk::Careful => ("Careful", c.log_warn),
        GuideRisk::Destructive => ("Can't be undone", c.error_text),
        GuideRisk::None => return None,
    };
    Some(
        // 1 px more below the text than above before; half a pixel each way now.
        div()
            .pad(px(0.5), px(7.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(colour)
            .text_size(theme.text.small)
            .line_height(theme.text.small * 1.4)
            .text_color(colour)
            .child(text),
    )
}

/// A glyph key of `Themes/Icons.xaml` the guide uses, by its code point.
pub fn glyph(key: &str) -> Option<Icon> {
    Some(match key {
        "GlyphAdd" => Icon::Add,
        "GlyphFolderOpen" => Icon::FolderOpen,
        "GlyphPlay" => Icon::Play,
        "GlyphCopy" => Icon::Copy,
        "GlyphInfo" => Icon::Info,
        "GlyphList" => Icon::List,
        "GlyphGlobe" => Icon::Globe,
        "GlyphDatabase" => Icon::Database,
        "GlyphContainer" => Icon::Container,
        "GlyphWarning" => Icon::Warning,
        "GlyphDownload" => Icon::Download,
        "GlyphFolder" => Icon::Folder,
        "GlyphKeyboard" => Icon::Keyboard,
        "GlyphSettings" => Icon::Settings,
        "GlyphShow" => Icon::Show,
        "GlyphSearch" => Icon::Search,
        "GlyphQuestion" => Icon::Question,
        "GlyphCancel" => Icon::Close,
        "GlyphRefresh" => Icon::Refresh,
        "GlyphChevronRight" => Icon::ChevronRight,
        "GlyphMore" => Icon::More,
        "GlyphStop" => Icon::Stop,
        "GlyphDelete" => Icon::Delete,
        "GlyphReset" => Icon::Reset,
        "GlyphCheck" => Icon::Check,
        "GlyphDash" => Icon::Dash,
        _ => return None,
    })
}

/// The icon an item names, drawn as the window draws it.
fn item_icon(key: &'static str, cx: &App) -> gpui::AnyElement {
    let theme = Theme::get(cx);
    let c = theme.colors;
    match key {
        "Dot.Running" => status_dot(c.success_text, true, px(10.0)).into_any_element(),
        "Dot.Stopped" => status_dot(c.text_muted, false, px(10.0)).into_any_element(),
        "Dot.Busy" => status_dot(c.log_warn, true, px(10.0)).into_any_element(),
        "Dot.None" => icon(Icon::Dash, c.text_muted, cx).into_any_element(),
        "FlameFilled" => flame(c.keep_warm_fg).into_any_element(),
        "LayoutPanel" => icons::layout_icon(LayoutIcon::Panel, false, c.text_primary).into_any_element(),
        number if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) => div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(14.0))
            .text_color(c.accent)
            .child(number)
            .into_any_element(),
        other => icon(glyph(other).unwrap_or(Icon::Info), c.text_primary, cx).into_any_element(),
    }
}

/// `FlameFilled` of `Themes/Icons.xaml` (16 × 16 path data), filled in KeepWarmFg, 14 px as the guide draws it.
const FLAME_FILLED: &str = "M4.11056 6.18914L4.10959 6.19037L4.10785 6.19258L4.1029 6.19894L4.08731 6.21936C4.07451 6.23635 4.057 6.26009 4.03559 6.29026C3.99281 6.35055 3.93432 6.43679 3.86679 6.54649C3.732 6.76546 3.55959 7.08054 3.40429 7.47178C3.0944 8.25245 2.84483 9.35744 3.12312 10.6086C3.38774 11.7982 3.89791 12.9035 4.78195 13.7126C5.67415 14.5292 6.89927 15 8.50338 15C10.1211 15 11.3875 14.4571 12.2463 13.5266C13.0989 12.6027 13.5034 11.3471 13.5034 10C13.5034 9.14123 13.2179 8.31144 12.8341 7.53604C12.4688 6.7979 11.9948 6.07168 11.5492 5.38919C11.5274 5.35579 11.5057 5.32249 11.484 5.2893C11.0111 4.56435 10.5805 3.89406 10.2945 3.25635C10.0087 2.61894 9.89683 2.07627 9.99247 1.59806C10.0219 1.45117 9.98382 1.29885 9.88885 1.18301C9.79389 1.06716 9.65198 1 9.50218 1C9.08022 1 8.377 1.14838 7.70997 1.49339C7.03689 1.84154 6.33312 2.42606 6.02784 3.34189C5.63773 4.51222 6.08683 5.83198 6.47104 6.66488C6.64207 7.03564 6.49357 7.44529 6.20976 7.5872C5.94247 7.72084 5.61745 7.6125 5.48381 7.34521L4.9494 6.27639C4.87302 6.12365 4.72397 6.02044 4.55412 6.0027C4.38427 5.98497 4.21685 6.05547 4.11056 6.18914Z";

/// One command of an SVG path with absolute coordinates: `M`, `L`, `C`, `Z` - all the flame uses.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PathCommand {
    Move(f32, f32),
    Line(f32, f32),
    Cubic([f32; 6]),
    Close,
}

fn parse_path(data: &str) -> Vec<PathCommand> {
    let mut out = Vec::new();
    let mut command = ' ';
    let mut numbers: Vec<f32> = Vec::new();
    let flush = |command: char, numbers: &mut Vec<f32>, out: &mut Vec<PathCommand>| {
        let need = match command {
            'M' | 'L' => 2,
            'C' => 6,
            _ => 0,
        };
        if need > 0 {
            for (n, chunk) in numbers.chunks_exact(need).enumerate() {
                out.push(match (command, n) {
                    ('M', 0) => PathCommand::Move(chunk[0], chunk[1]),
                    ('M' | 'L', _) => PathCommand::Line(chunk[0], chunk[1]),
                    _ => PathCommand::Cubic([chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5]]),
                });
            }
        }
        numbers.clear();
    };
    let mut token = String::new();
    for ch in data.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && token.is_empty()) {
            token.push(ch);
            continue;
        }
        if !token.is_empty() {
            numbers.extend(token.parse::<f32>().ok());
            token.clear();
        }
        if ch == '-' {
            token.push(ch);
        } else if ch.is_ascii_alphabetic() {
            flush(command, &mut numbers, &mut out);
            command = ch.to_ascii_uppercase();
            if command == 'Z' {
                out.push(PathCommand::Close);
            }
        }
    }
    flush(command, &mut numbers, &mut out);
    out
}

fn flame(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            // The path spans x 3 to 13.5 and y 1 to 15: Stretch="Uniform" in 14 × 14 centres it across.
            let o = bounds.origin;
            let p = |x: f32, y: f32| point(o.x + px(x - 3.0 + 1.75), o.y + px(y - 1.0));
            let mut b = PathBuilder::fill();
            for command in parse_path(FLAME_FILLED) {
                match command {
                    PathCommand::Move(x, y) => b.move_to(p(x, y)),
                    PathCommand::Line(x, y) => b.line_to(p(x, y)),
                    PathCommand::Cubic([x1, y1, x2, y2, x, y]) => b.cubic_bezier_to(p(x, y), p(x1, y1), p(x2, y2)),
                    PathCommand::Close => b.close(),
                }
            }
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(14.0))
}

impl Focusable for GuideDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GuideDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let several = self.steps.len() > 1;
        let last = self.index + 1 >= self.steps.len();
        let step = self.steps.get(self.index).copied();
        // A short screen (a 768 px laptop) keeps the buttons on it.
        let height = (window.viewport_size().height - px(40.0)).min(px(660.0));
        let width = if several { 900.0 } else { 760.0 };

        let next_label = if last { self.finish_text.clone() } else { "Next".into() };
        let mut next = button("guide-next", next_label, ButtonKind::Primary, true, cx)
            .min_w(px(84.0))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next(cx)));
        if !last {
            next = next.tip("The next page (Enter)");
        }
        let bar = div()
            .flex()
            .items_center()
            .justify_between()
            .pad(px(12.0), px(20.0))
            .border_t_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        link("guide-skip", self.skip_text.clone(), cx)
                            .tip("Close the guide (Esc) - the ? at the top right opens it again")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.skip(cx))),
                    )
                    .when(several, |d| {
                        d.child(
                            div().text_size(theme.text.caption).text_color(c.text_muted).child(format!(
                                "Step {} of {}",
                                self.index + 1,
                                self.steps.len()
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .when(several, |d| {
                        d.child(
                            button("guide-back", "Back", ButtonKind::Secondary, self.index > 0, cx)
                                .min_w(px(84.0))
                                .tip("The previous page")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.back(cx))),
                        )
                    })
                    .when(last && self.offer_tour, |d| {
                        d.child(
                            button("guide-tour", "Take the tour", ButtonKind::Secondary, true, cx)
                                .tip("Close this guide and point at each part of the window")
                                .on_click(
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.finish(GuideResult::Tour, cx)),
                                ),
                        )
                    })
                    .child(next),
            );

        div()
            .id("guide")
            .key_context("GuideDialog")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &GuideNext, _, cx| this.next(cx)))
            .on_action(cx.listener(|this, _: &GuideSkip, _, cx| this.skip(cx)))
            .w(px(width))
            .h(height)
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(c.card_border)
            .bg(c.window_bg)
            .shadow_xl()
            .overflow_hidden()
            .font_family(theme.font.clone())
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            // With one page there is no step list to carry the title: it heads the page, as the window title did.
            .when(!several, |d| {
                d.child(
                    // 16 px above the title (and 16 below, so the page under it has 12 of its own).
                    div()
                        .pad(px(16.0), px(20.0))
                        .text_size(theme.text.heading)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.title.clone()),
                )
            })
            .child(
                div().flex().flex_1().min_h_0().when(several, |d| d.child(self.render_steps(cx))).child(
                    div()
                        .id("guide-page")
                        .flex_1()
                        .min_w_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .when_some(step, |d, step| d.child(self.render_page(step, cx))),
                ),
            )
            .child(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::onboarding::*;
    use super::*;
    use dnnm_infrastructure::app_db::AppDatabase;

    #[test]
    fn the_guide_has_eight_pages_in_order() {
        let short: Vec<&str> = GUIDE.iter().map(|s| s.short_title).collect();
        assert_eq!(
            short,
            [
                "Welcome",
                "Before you start",
                "Finding your way",
                "Your websites",
                "Buttons and actions",
                "Logs and troubleshooting",
                "Updates",
                "Ready"
            ]
        );
        assert_eq!(GUIDE[4].sections[0].items[3].risk, GuideRisk::Safe);
        assert_eq!(
            GUIDE[1].sections[1].items[0].text,
            "Where the files of new websites are stored - C:\\DNN unless you choose another. **Settings → Projects**."
        );
    }

    #[test]
    fn continued_lines_keep_one_space() {
        assert_eq!(
            WELCOME.intro,
            "DNN Manager runs DNN websites on this computer, so you can build and try them out before they go live. DNN (also called DotNetNuke) is a system for building websites."
        );
        assert!(ACTIONS.sections[0].items[3].text.contains("10 seconds. Works while"));
        assert!(TROUBLESHOOTING.sections[0].items[0].text.contains("says what to do. A red dot"));
    }

    #[test]
    fn every_icon_is_known() {
        let steps = GUIDE.iter().chain([&DETAILS, &NEW_PROJECT, &SETTINGS]);
        for step in steps {
            for item in step.sections.iter().flat_map(|s| s.items) {
                let known = glyph(item.icon).is_some()
                    || item.icon.starts_with("Dot.")
                    || matches!(item.icon, "FlameFilled" | "LayoutPanel")
                    || item.icon.bytes().all(|b| b.is_ascii_digit());
                assert!(known, "{}", item.icon);
            }
        }
    }

    #[test]
    fn help_for_each_page() {
        assert_eq!(
            help_for(HelpPage::Projects).iter().map(|s| s.title).collect::<Vec<_>>(),
            ["Your websites", "Buttons and what they do"]
        );
        assert_eq!(help_for(HelpPage::Details)[0].title, "A website's details");
        assert_eq!(help_for(HelpPage::Setup)[0].title, "New project");
        assert_eq!(help_for(HelpPage::Settings)[0].title, "Settings");
        assert_eq!(help_for(HelpPage::Troubleshoot)[0].title, "When something goes wrong");
    }

    #[test]
    fn at_start_decides_as_onboarding_does() {
        assert_eq!(at_start(0, true, &GUIDE, 1).0, GuideAtStart::Welcome);
        assert_eq!(at_start(0, true, &GUIDE, 1).1.len(), 8);
        assert_eq!(at_start(0, false, &GUIDE, 1).0, GuideAtStart::Nothing);
        assert_eq!(at_start(1, true, &GUIDE, 1).0, GuideAtStart::Nothing);
        // A guide 2 whose pages are all from 1: nothing new.
        assert_eq!(at_start(1, false, &GUIDE, 2).0, GuideAtStart::Nothing);
    }

    #[test]
    fn the_flame_path_parses() {
        let commands = parse_path(FLAME_FILLED);
        assert_eq!(commands.first(), Some(&PathCommand::Move(4.11056, 6.18914)));
        assert_eq!(commands.last(), Some(&PathCommand::Close));
        assert!(commands.iter().filter(|c| matches!(c, PathCommand::Cubic(_))).count() > 20);
    }

    #[test]
    fn onboarding_state_round_trips_with_the_wpf_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDatabase::new(dir.path().join("dnnmanager.db"));
        assert_eq!(load(&db), OnboardingState::default());

        // The very first start: the guide, the first version noted, the user new until an update.
        assert_eq!(guide_at_start(&db, true).0, GuideAtStart::Welcome);
        assert!(note_first_version(&db, true, "1.8.7"));
        save_guide_seen(&db, true).unwrap();
        assert_eq!(db.state("onboarding", "guideVersion").unwrap().as_deref(), Some("1"));
        assert_eq!(db.state("onboarding", "skipped").unwrap().as_deref(), Some("true"));
        assert_eq!(db.state("onboarding", "firstVersion").unwrap().as_deref(), Some("1.8.7"));
        assert_eq!(guide_at_start(&db, false).0, GuideAtStart::Nothing);
        assert!(!note_first_version(&db, false, "1.8.8"));

        // Someone who used DNN Manager before the guide: not shown, but remembered.
        let dir2 = tempfile::tempdir().unwrap();
        let old = AppDatabase::new(dir2.path().join("dnnmanager.db"));
        old.set_state("onboarding", "skipped", "True").unwrap();
        assert_eq!(guide_at_start(&old, false).0, GuideAtStart::Nothing);
        assert_eq!(load(&old), OnboardingState { guide_version: 1, skipped: true, first_version: None });
        assert!(!note_first_version(&old, false, "1.8.7"));
    }
}
