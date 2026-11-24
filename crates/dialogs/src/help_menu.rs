//! The title bar's Help menu (the ?) - `MainWindow.Help.cs`'s `Help_Click`: this page's help, the guide, the tour, and
//! where to read more - as data; the app draws it as its other menus and runs the command an item names. Also the
//! names of the pages' help (`HelpPageName`) and the addresses on GitHub.

/// DNN Manager's repository (`AppReleaseFeed.Repository`).
pub const REPOSITORY: &str = "https://github.com/Albadit/DnnManager.NET";
/// User Guide on GitHub (`OpenUserGuide`).
pub const USER_GUIDE_URL: &str = "https://github.com/Albadit/DnnManager.NET/blob/main/.docs/user-guide.md";
/// What's new's "All releases on GitHub".
pub const RELEASES_URL: &str = "https://github.com/Albadit/DnnManager.NET/releases";

/// The title bar's ? button: its tooltip.
pub const HELP_BUTTON_TOOLTIP: &str = "Help - what this page is for, the getting started guide and the tour";

/// The page the help is for (`MainWindow.HelpPage`): Settings or Troubleshoot when one is open over the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpPage {
    Projects,
    /// A project's details, open on Projects.
    Details,
    /// New project (`SetupPage`).
    Setup,
    SourceControl,
    Settings,
    Troubleshoot,
}

impl HelpPage {
    /// The key `Onboarding.HelpFor` takes.
    pub fn key(self) -> &'static str {
        match self {
            HelpPage::Projects => "Projects",
            HelpPage::Details => "Details",
            HelpPage::Setup => "Setup",
            HelpPage::SourceControl => "SourceControl",
            HelpPage::Settings => "Settings",
            HelpPage::Troubleshoot => "Troubleshoot",
        }
    }

    /// The page's name in the menu and the help's title (`HelpPageName`).
    pub fn name(self) -> &'static str {
        match self {
            HelpPage::Details => "Project details",
            HelpPage::Setup => "New project",
            HelpPage::SourceControl => "Source Control",
            other => other.key(),
        }
    }
}

/// What a Help menu item does - the command id it runs (`AppCommands`), for its shortcut and the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpCommand {
    /// `help.page` (F1): this page's help - [`GuideDialog::page_help`](crate::guide::GuideDialog::page_help).
    PageHelp,
    /// `help.gettingStarted`: [`GuideDialog::getting_started`](crate::guide::GuideDialog::getting_started).
    GettingStarted,
    /// `help.tour`: the tour (`crate::tour`).
    Tour,
    /// `settings.keyboard`: Settings → Keyboard shortcuts.
    KeyboardShortcuts,
    /// `app.whatsNew`: [`WhatsNewDialog`](crate::whats_new::WhatsNewDialog) with [`latest`](crate::whats_new::latest).
    WhatsNew,
    /// `help.userGuide`: opens [`USER_GUIDE_URL`].
    UserGuide,
}

impl HelpCommand {
    pub fn id(self) -> &'static str {
        match self {
            HelpCommand::PageHelp => "help.page",
            HelpCommand::GettingStarted => "help.gettingStarted",
            HelpCommand::Tour => "help.tour",
            HelpCommand::KeyboardShortcuts => "settings.keyboard",
            HelpCommand::WhatsNew => "app.whatsNew",
            HelpCommand::UserGuide => "help.userGuide",
        }
    }

    /// The default shortcut shown on the right of the item (`AppCommands`; only this page's help has one).
    pub fn default_shortcut(self) -> Option<&'static str> {
        (self == HelpCommand::PageHelp).then_some("F1")
    }
}

/// One item of the menu; `separator_before` draws a line above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpMenuItem {
    pub label: String,
    pub command: HelpCommand,
    pub separator_before: bool,
}

/// The menu for `page`. What's New isn't offered while the user is still on the version DNN Manager first started as
/// (`IsNewUser`, see [`crate::guide::onboarding::is_new_user`]).
pub fn items(page: HelpPage, is_new_user: bool) -> Vec<HelpMenuItem> {
    let item = |label: String, command, separator_before| HelpMenuItem { label, command, separator_before };
    let mut items = vec![
        item(format!("Help for This Page ({})", page.name()), HelpCommand::PageHelp, false),
        item("Getting Started Guide".into(), HelpCommand::GettingStarted, false),
        item("Take the Tour".into(), HelpCommand::Tour, false),
        item("Keyboard Shortcuts".into(), HelpCommand::KeyboardShortcuts, true),
    ];
    if !is_new_user {
        items.push(item("What's New in This Version".into(), HelpCommand::WhatsNew, false));
    }
    items.push(item("User Guide on GitHub".into(), HelpCommand::UserGuide, false));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_as_the_wpf_app_builds_it() {
        let labels: Vec<String> = items(HelpPage::Setup, false).into_iter().map(|i| i.label).collect();
        assert_eq!(
            labels,
            [
                "Help for This Page (New project)",
                "Getting Started Guide",
                "Take the Tour",
                "Keyboard Shortcuts",
                "What's New in This Version",
                "User Guide on GitHub"
            ]
        );
        let new_user = items(HelpPage::Details, true);
        assert_eq!(new_user[0].label, "Help for This Page (Project details)");
        assert!(new_user.iter().all(|i| i.command != HelpCommand::WhatsNew));
        assert!(new_user[3].separator_before && new_user[3].command == HelpCommand::KeyboardShortcuts);
        assert_eq!(HelpCommand::PageHelp.default_shortcut(), Some("F1"));
    }
}
