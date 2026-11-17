//! Help (`MainWindow.Help.cs`): the title bar's ? menu, this page's help (F1), the getting started guide, the tour,
//! What's new and the user guide on GitHub - and, at the start, What's new after a newer version and the guide on the
//! very first start.

use gpui::{AppContext, Context, Entity, EventEmitter, Pixels, Point, Render, Subscription, Window};

use dnnm_dialogs::guide::{GuideAtStart, GuideDialog, GuideResult, onboarding};
use dnnm_dialogs::help_menu::{self, HelpCommand, HelpPage};
use dnnm_dialogs::setting_rules::Category;
use dnnm_dialogs::tour::{self, GuidedTour, TourEnter, TourEvent, TourLayout};
use dnnm_dialogs::whats_new::{self, OpenLink, Version, WhatsNewDialog};
use dnnm_ui::menu::{ContextMenu, MenuItem};

use super::modals::Modal;
use super::{Page, Workspace};
use crate::store::ToastKind;

/// This build's version - the WPF release it is a port of.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// State area `version`, row `lastRun`: the version that ran last (`VersionState`) - shared with the WPF app.
const VERSION_AREA: &str = "version";
const LAST_RUN: &str = "lastRun";

pub(crate) fn current_version() -> Version {
    Version::parse(VERSION).unwrap_or(Version::new(0, 0, 0))
}

impl Workspace {
    /// Still on the version DNN Manager first started as: What's new isn't offered (`IsNewUser`).
    pub(crate) fn is_new_user(&self, cx: &gpui::App) -> bool {
        onboarding::is_new_user(&onboarding::load(self.store.read(cx).db()), VERSION)
    }

    /// The page the help is for - Settings or Troubleshoot when one is open over the page (`HelpPage`).
    pub(crate) fn help_page(&self, cx: &gpui::App) -> HelpPage {
        match &self.settings {
            Some(Modal::Settings(_, _)) => HelpPage::Settings,
            Some(Modal::Troubleshoot(_, _)) => HelpPage::Troubleshoot,
            None => match self.page {
                Page::NewProject => HelpPage::Setup,
                Page::Projects if self.projects.read(cx).details_of(cx).is_some() => HelpPage::Details,
                Page::Projects => HelpPage::Projects,
            },
        }
    }

    pub(crate) fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Err(e) = dnnm_infrastructure::shell::open_url(url) {
            self.store.update(cx, |s, cx| s.toast(ToastKind::Error, format!("The link couldn't be opened: {e}"), false, cx));
        }
    }

    /// Shows a view over the window that emits `E`; `on_event` gets each one and says whether it closes the view.
    pub(crate) fn show_view<V, E>(
        &mut self,
        view: Entity<V>,
        on_event: impl Fn(&mut Workspace, &E, &mut Window, &mut Context<Workspace>) -> bool + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        V: Render + EventEmitter<E>,
        E: 'static,
    {
        self.menu = None;
        self.menu = None;
        let sub: Subscription = cx.subscribe_in(&view, window, move |ws, _, e: &E, window, cx| {
            if on_event(ws, e, window, cx) {
                ws.dialog = None;
                if ws.tour.is_none() {
                    window.focus(&ws.focus);
                }
            }
            cx.notify();
        });
        self.dialog = Some((view.into(), sub));
        cx.notify();
    }

    /// The getting started guide (`ShowGuide`) - from Help, Settings → Help or the palette.
    pub(crate) fn show_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| GuideDialog::welcome(window, cx));
        self.show_guide_view(view, window, cx);
    }

    fn show_guide_view(&mut self, view: Entity<GuideDialog>, window: &mut Window, cx: &mut Context<Self>) {
        self.show_view(
            view,
            |ws, result: &GuideResult, window, cx| {
                let db = ws.store.read(cx).db().clone();
                let _ = onboarding::save_guide_seen(&db, *result == GuideResult::Skipped);
                if *result == GuideResult::Tour {
                    let weak = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |ws, cx| ws.start_tour(window, cx));
                    });
                }
                true
            },
            window,
            cx,
        );
    }

    /// What the page shown is for (F1, `ShowPageHelp`).
    pub(crate) fn show_page_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.help_page(cx);
        let view = cx.new(|cx| GuideDialog::page_help(page, window, cx));
        self.show_view(view, |_, _: &GuideResult, _, _| true, window, cx);
    }

    /// What's new in this version (`ShowWhatsNew`): its release notes.
    pub(crate) fn show_whats_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notes = whats_new::latest(current_version());
        self.show_notes(notes, window, cx, |_, _, _| {});
    }

    /// Release notes in the What's new dialog, then `then` once it is closed; nothing (and `then` at once) without notes.
    fn show_notes(
        &mut self,
        notes: Vec<whats_new::ReleaseNote>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) {
        if notes.is_empty() {
            then(self, window, cx);
            return;
        }
        let view = cx.new(|cx| WhatsNewDialog::new(notes, window, cx));
        let links = cx.subscribe_in(&view, window, |ws, _, link: &OpenLink, _, cx| ws.open_link(&link.0, cx));
        self._subscriptions.push(links);
        // The follow-up (the guide at the start) runs once the dialog has gone - one dialog at a time.
        self.after_notes = Some(Box::new(then));
        self.show_view(
            view,
            |ws, _: &dnnm_ui::dialog::DialogEvent<()>, window, cx| {
                if let Some(then) = ws.after_notes.take() {
                    let weak = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |ws, cx| then(ws, window, cx));
                    });
                }
                true
            },
            window,
            cx,
        );
    }

    /// The title bar's ? (`Help_Click`): this page's help, the guide, the tour, then where to read more.
    pub(crate) fn open_help_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let mut items = Vec::new();
        for item in help_menu::items(self.help_page(cx), self.is_new_user(cx)) {
            if item.separator_before {
                items.push(MenuItem::Separator);
            }
            let (weak, command) = (weak.clone(), item.command);
            let keys = super::shortcuts::shortcut_text(command.id(), &self.store.read(cx).settings);
            items.push(
                MenuItem::new(item.label, move |window, cx| {
                    let _ = weak.update(cx, |ws, cx| {
                        ws.menu = None;
                        ws.run_help(command, window, cx);
                    });
                })
                .shortcut(keys),
            );
        }
        self.menu = None;
        self.menu = Some(ContextMenu::new(position, items));
        cx.notify();
    }

    pub(crate) fn run_help(&mut self, command: HelpCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            HelpCommand::PageHelp => self.show_page_help(window, cx),
            HelpCommand::GettingStarted => self.show_guide(window, cx),
            HelpCommand::Tour => self.start_tour(window, cx),
            HelpCommand::KeyboardShortcuts => self.open_settings_on(Some(Category::Keyboard), window, cx),
            HelpCommand::WhatsNew => self.show_whats_new(window, cx),
            HelpCommand::UserGuide => self.open_link(help_menu::USER_GUIDE_URL, cx),
        }
    }

    /// The tour of the window (`StartTour`): Settings or Troubleshoot close first (asking about unsaved settings); the
    /// panel is put back as it was afterwards.
    pub(crate) fn start_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tour.is_some() {
            return;
        }
        self.close_modal_then(window, cx, |ws, window, cx| {
            ws.palette = None;
            ws.menu = None;
            ws.menu = None;
            ws.panel_before_tour = ws.panel_visible;
            let layout = TourLayout {
                sidebar_visible: ws.sidebar_visible,
                command_center_visible: true,
                update_visible: ws.update_available.is_some(),
                status_bar_visible: ws.store.read(cx).settings.status_bar_visible(),
            };
            let targets = ws.tour_targets.clone();
            let view = cx.new(|cx| GuidedTour::new(tour::stops(layout), targets, window, cx));
            let sub = cx.subscribe_in(&view, window, |ws, _, e: &TourEvent, window, cx| {
                match e {
                    TourEvent::Enter(TourEnter::ShowProjectsTable) => {
                        ws.page = Page::Projects;
                        ws.store.update(cx, |s, _| s.projects_shown = true);
                        ws.projects.update(cx, |p, cx| p.close_details(cx));
                    }
                    TourEvent::Enter(TourEnter::OpenPanel) => ws.panel_visible = true,
                    TourEvent::Ended { .. } => {
                        ws.tour = None;
                        if !ws.panel_before_tour {
                            ws.panel_visible = false;
                        }
                        window.focus(&ws.focus);
                    }
                }
                cx.notify();
            });
            ws.tour = Some((view, sub));
            cx.notify();
        });
    }

    /// At the start, once the window is up (`RestoreWorkspace`): What's new after a newer version - not for a new user -
    /// then the guide on the very first start, or the pages a newer guide added.
    pub(crate) fn help_at_start(&mut self, first_start: bool, window: &mut Window, cx: &mut Context<Self>) {
        let db = self.store.read(cx).db().clone();
        let new_user = onboarding::note_first_version(&db, first_start, VERSION);
        let last_run = db.state(VERSION_AREA, LAST_RUN).ok().flatten();
        let current = current_version();
        let since = last_run
            .as_deref()
            .and_then(Version::parse)
            .filter(|before| !new_user && current.is_newer_than(before));
        if last_run.as_deref() != Some(VERSION) {
            let _ = db.set_state(VERSION_AREA, LAST_RUN, VERSION);
        }
        let notes = since.map(|since| whats_new::between(Some(since), current)).unwrap_or_default();
        self.show_notes(notes, window, cx, move |ws, window, cx| ws.guide_at_start(first_start, window, cx));
    }

    fn guide_at_start(&mut self, first_start: bool, window: &mut Window, cx: &mut Context<Self>) {
        let db = self.store.read(cx).db().clone();
        let (what, steps) = onboarding::guide_at_start(&db, first_start);
        let view = match what {
            GuideAtStart::Nothing => return,
            GuideAtStart::Welcome => cx.new(|cx| GuideDialog::welcome(window, cx)),
            GuideAtStart::NewSteps => cx.new(|cx| GuideDialog::new_steps(steps, window, cx)),
        };
        self.show_guide_view(view, window, cx);
    }
}
