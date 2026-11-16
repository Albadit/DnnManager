//! Ports `Presentation/Services/SsmsConnectDialog.cs`: signs SQL Server Management Studio 21+ in to a database by
//! completing its Connect dialog through UI Automation.
//!
//! SSMS takes server, database and login on its command line but never a password - and with any of those switches it
//! connects straight away, fails without the password and shows an error first. Started with no connection switches it
//! just opens the Connect dialog, so that is filled in here instead: server, SQL Server Authentication, login, password
//! (remembered), database, trust-server-certificate and name, then Connect. Only the SSMS process DNN Manager just
//! started is touched. Fields are found by name, the password box by its role and the Connect button / authentication
//! choice by language-neutral ids.
//!
//! Every call blocks (up to its timeout) and runs its UI Automation on a thread of its own, in the multithreaded COM
//! apartment UI Automation clients use.

use std::time::{Duration, Instant};

use dnnm_sql::model::SiteSqlConnection;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_Expanded, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTogglePattern, IUIAutomationValuePattern, IUIAutomationWindowPattern, ToggleState_On,
    TreeScope_Children, TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID,
    UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_EditControlTypeId,
    UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_IsPasswordPropertyId, UIA_ListItemControlTypeId,
    UIA_NamePropertyId, UIA_ProcessIdPropertyId, UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
    UIA_WindowControlTypeId, UIA_WindowPatternId, WindowVisualState_Minimized, WindowVisualState_Normal,
};
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::core::BSTR;

use crate::ssms::{ExitWatch, SsmsProcess};

/// What [`sign_in`] fills in.
pub struct SignIn<'a> {
    pub database: &'a SiteSqlConnection,
    pub trust_server_certificate: bool,
    pub display_name: &'a str,
    /// Tick SSMS's "Remember Password" (untick it when false).
    pub remember_password: bool,
    pub timeout: Duration,
}

/// Waits for `ssms`'s Connect dialog and signs in to the database. False when the dialog didn't show up in time (SSMS
/// set not to show it at startup) or didn't look as expected.
pub fn sign_in(ssms: &SsmsProcess, request: &SignIn<'_>) -> bool {
    let (id, handle) = (ssms.id, ssms.watch());
    let database = request.database.clone();
    let (trust, name, remember, timeout) = (
        request.trust_server_certificate,
        request.display_name.to_string(),
        request.remember_password,
        request.timeout,
    );
    crate::win::on_com_thread(COINIT_MULTITHREADED, move || {
        let ui = Ui::new()?;
        let dialog = ui.wait_for_dialog(id, &handle, timeout)?;
        // Authentication first: it decides which of the other fields are enabled.
        ui.select_authentication(&dialog, "SqlPassword")?;
        ui.set_text(&dialog, "Server Name", &database.server)?;
        ui.set_text(&dialog, "User Name", &database.user)?;
        // SAFETY (this block and the rest): UI Automation calls on this thread's apartment.
        let password_box = unsafe {
            let password = ui.condition(UIA_IsPasswordPropertyId.0, VARIANT::from(true))?;
            let edit = ui.condition(UIA_ControlTypePropertyId.0, VARIANT::from(UIA_EditControlTypeId.0))?;
            dialog.FindFirst(TreeScope_Descendants, &ui.automation.CreateAndCondition(&password, &edit).ok()?).ok()?
        };
        let value: IUIAutomationValuePattern = unsafe { password_box.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
        unsafe { value.SetValue(&BSTR::from(database.password.as_str())) }.ok()?;

        ui.set_checked(&dialog, "Remember Password", remember);
        ui.set_checked(&dialog, "Trust Server Certificate", trust);
        ui.set_database(&dialog, if database.database.is_empty() { "<default>" } else { &database.database });
        let _ = ui.set_text(&dialog, "Name", &name);

        let connect = unsafe {
            dialog
                .FindFirst(
                    TreeScope_Descendants,
                    &ui.condition(UIA_AutomationIdPropertyId.0, VARIANT::from("OkButton"))?,
                )
                .ok()?
        };
        let invoke: IUIAutomationInvokePattern = unsafe { connect.GetCurrentPatternAs(UIA_InvokePatternId) }.ok()?;
        unsafe { invoke.Invoke() }.ok()?;
        Some(())
    })
    .flatten()
    .is_some()
}

/// Brings an SSMS that is already running to the front and opens its Connect dialog (Object Explorer's "Connect Object
/// Explorer..."), so a new connection goes into that window instead of a new SSMS. An open Connect dialog is reused.
/// False when the dialog doesn't appear - e.g. SSMS is busy with another dialog.
pub fn open_in_running(ssms: &SsmsProcess) -> bool {
    let Some(window) = ssms.main_window else { return false };
    let (id, handle) = (ssms.id, ssms.watch());
    crate::win::on_com_thread(COINIT_MULTITHREADED, move || {
        let ui = Ui::new()?;
        let hwnd = HWND(window as *mut _);
        // SAFETY: UI Automation and window calls on this thread with a window handle that may have gone (then they fail).
        unsafe {
            let main = ui.automation.ElementFromHandle(hwnd).ok()?;
            if let Ok(pattern) = main.GetCurrentPatternAs::<IUIAutomationWindowPattern>(UIA_WindowPatternId)
                && pattern.CurrentWindowVisualState().is_ok_and(|s| s == WindowVisualState_Minimized)
            {
                let _ = pattern.SetWindowVisualState(WindowVisualState_Normal);
            }
            let _ = SetForegroundWindow(hwnd);

            if ui.wait_for_dialog(id, &handle, Duration::ZERO).is_some() {
                return Some(());
            }
            let connect = ui.find(&main, "Connect Object Explorer...", UIA_ButtonControlTypeId)?;
            let invoke: IUIAutomationInvokePattern = connect.GetCurrentPatternAs(UIA_InvokePatternId).ok()?;
            invoke.Invoke().ok()?;
        }
        ui.wait_for_dialog(id, &handle, Duration::from_secs(5)).map(|_| ())
    })
    .flatten()
    .is_some()
}

struct Ui {
    automation: IUIAutomation,
}

impl Ui {
    fn new() -> Option<Self> {
        // SAFETY: created on the calling thread's (multithreaded) apartment.
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .ok()
            .map(|automation| Self { automation })
    }

    fn condition(&self, property: i32, value: VARIANT) -> Option<IUIAutomationCondition> {
        use windows::Win32::UI::Accessibility::UIA_PROPERTY_ID;
        // SAFETY: a plain UI Automation call; the value outlives it.
        unsafe { self.automation.CreatePropertyCondition(UIA_PROPERTY_ID(property), &value) }.ok()
    }

    /// The first descendant of `parent` named `name` of control type `kind`.
    fn find(
        &self,
        parent: &IUIAutomationElement,
        name: &str,
        kind: UIA_CONTROLTYPE_ID,
    ) -> Option<IUIAutomationElement> {
        let by_name = self.condition(UIA_NamePropertyId.0, VARIANT::from(name))?;
        let by_type = self.condition(UIA_ControlTypePropertyId.0, VARIANT::from(kind.0))?;
        // SAFETY: plain UI Automation calls.
        unsafe {
            let both = self.automation.CreateAndCondition(&by_name, &by_type).ok()?;
            parent.FindFirst(TreeScope_Descendants, &both).ok()
        }
    }

    /// The Connect window of the process - on its own or inside the main window.
    fn wait_for_dialog(&self, id: u32, ssms: &ExitWatch, timeout: Duration) -> Option<IUIAutomationElement> {
        let own_windows = self.condition(UIA_ProcessIdPropertyId.0, VARIANT::from(id as i32))?;
        let deadline = Instant::now() + timeout;
        // A zero timeout still looks once.
        loop {
            // A window came or went mid-scan, or UI Automation hiccupped while SSMS starts: an error is looked again.
            if let Some(found) = self.scan(&own_windows) {
                return Some(found);
            }
            if Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(500));
            }
            if Instant::now() >= deadline || ssms.has_exited() {
                return None;
            }
        }
    }

    fn scan(&self, own_windows: &IUIAutomationCondition) -> Option<IUIAutomationElement> {
        // SAFETY: plain UI Automation calls.
        unsafe {
            let windows = self.automation.GetRootElement().ok()?.FindAll(TreeScope_Children, own_windows).ok()?;
            for index in 0..windows.Length().ok()? {
                let Ok(window) = windows.GetElement(index) else { continue };
                if window.CurrentName().is_ok_and(|n| n == "Connect") {
                    return Some(window);
                }
                if let Some(inner) = self.find(&window, "Connect", UIA_WindowControlTypeId) {
                    return Some(inner);
                }
            }
        }
        None
    }

    /// Picks the authentication whose item is `[key, label]` - the key doesn't depend on the language.
    fn select_authentication(&self, dialog: &IUIAutomationElement, key: &str) -> Option<()> {
        let combo = self.find(dialog, "Authentication", UIA_ComboBoxControlTypeId)?;
        // SAFETY: plain UI Automation calls.
        unsafe {
            let expand =
                combo.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).ok();
            if let Some(expand) = &expand {
                let _ = expand.Expand();
            }
            let selected = (|| {
                let items = combo
                    .FindAll(
                        TreeScope_Descendants,
                        &self.condition(UIA_ControlTypePropertyId.0, VARIANT::from(UIA_ListItemControlTypeId.0))?,
                    )
                    .ok()?;
                let prefix = format!("[{key},");
                for index in 0..items.Length().ok()? {
                    let Ok(item) = items.GetElement(index) else { continue };
                    if !item.CurrentName().is_ok_and(|n| n.to_string().starts_with(&prefix)) {
                        continue;
                    }
                    let select: IUIAutomationSelectionItemPattern =
                        item.GetCurrentPatternAs(UIA_SelectionItemPatternId).ok()?;
                    return select.Select().ok();
                }
                None
            })();
            if let Some(expand) = &expand
                && expand.CurrentExpandCollapseState().is_ok_and(|s| s == ExpandCollapseState_Expanded)
            {
                let _ = expand.Collapse();
            }
            selected
        }
    }

    fn set_text(&self, dialog: &IUIAutomationElement, name: &str, value: &str) -> Option<()> {
        let field = self.find(dialog, name, UIA_EditControlTypeId)?;
        // SAFETY: plain UI Automation calls.
        unsafe {
            let pattern: IUIAutomationValuePattern = field.GetCurrentPatternAs(UIA_ValuePatternId).ok()?;
            pattern.SetValue(&BSTR::from(value)).ok()
        }
    }

    /// The database box is an editable combo box; its text part takes the name.
    fn set_database(&self, dialog: &IUIAutomationElement, database: &str) {
        let Some(combo) = self.find(dialog, "Database Name", UIA_ComboBoxControlTypeId) else { return };
        let Some(by_id) = self.condition(UIA_AutomationIdPropertyId.0, VARIANT::from("PART_EditableTextBox")) else {
            return;
        };
        // SAFETY: plain UI Automation calls.
        unsafe {
            if let Ok(text) = combo.FindFirst(TreeScope_Descendants, &by_id)
                && let Ok(pattern) = text.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                let _ = pattern.SetValue(&BSTR::from(database));
            }
        }
    }

    fn set_checked(&self, dialog: &IUIAutomationElement, name: &str, on: bool) {
        let Some(check) = self.find(dialog, name, UIA_CheckBoxControlTypeId) else { return };
        // SAFETY: plain UI Automation calls.
        unsafe {
            let Ok(toggle) = check.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) else {
                return;
            };
            if toggle.CurrentToggleState().is_ok_and(|s| (s == ToggleState_On) != on) {
                let _ = toggle.Toggle();
            }
        }
    }
}
