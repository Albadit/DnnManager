//! The deployment preview: what a transfer will do before it does anything - each file as a new upload, a
//! replacement on the server, a deletion there, a download (new here, or replacing the project's copy), or left alone
//! with why - with the protected ones named and the bytes to send. The page shows it; confirmed, the transfer runs
//! without asking each question again ([`super::sync::transfer`] with `previewed`).

use super::model::Environment;
use super::paths::{IgnoreRules, remote_path};
use super::sync::{Action, TransferItem, refusal};

/// What happens to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// To the server, where there is none yet.
    Upload,
    /// To the server, over its copy.
    Replace,
    /// Deleted on the server.
    Delete,
    /// To the project, where there is none yet.
    Download,
    /// To the project, over its copy (its changes there are lost).
    Overwrite,
}

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::Upload => "New on the server",
            Step::Replace => "Replaced on the server",
            Step::Delete => "Deleted on the server",
            Step::Download => "New in the project",
            Step::Overwrite => "Replaced in the project",
        }
    }

    /// Whether it changes or removes something that is there now.
    pub fn destructive(self) -> bool {
        matches!(self, Step::Replace | Step::Delete | Step::Overwrite)
    }
}

/// One file of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub path: String,
    pub step: Step,
    /// Bytes sent (an upload) or received (a download); 0 for a deletion.
    pub bytes: u64,
    /// One of the environment's protected paths.
    pub protected: bool,
}

/// What a transfer will do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    /// In the order they will be sent.
    pub planned: Vec<Planned>,
    /// Left alone, with why.
    pub refused: Vec<(String, String)>,
}

impl Preview {
    pub fn count(&self, step: Step) -> usize {
        self.planned.iter().filter(|p| p.step == step).count()
    }

    pub fn bytes(&self) -> u64 {
        self.planned.iter().map(|p| p.bytes).sum()
    }

    pub fn protected(&self) -> Vec<&Planned> {
        self.planned.iter().filter(|p| p.protected).collect()
    }

    /// Whether anything that is there now is changed or removed.
    pub fn destructive(&self) -> bool {
        self.planned.iter().any(|p| p.step.destructive())
    }

    pub fn is_empty(&self) -> bool {
        self.planned.is_empty()
    }
}

/// The preview of sending `items` to or from `env`'s server.
pub fn preview(env: &Environment, items: &[TransferItem]) -> Preview {
    let protected = IgnoreRules::protected(env);
    let mappings = env.effective_mappings();
    let mut out = Preview::default();
    for item in items {
        if let Some(why) = refusal(item) {
            out.refused.push((item.path.clone(), why));
            continue;
        }
        if remote_path(&mappings, &item.path).is_none() {
            out.refused.push((item.path.clone(), "no mapping holds it".into()));
            continue;
        }
        let (step, bytes) = match item.action {
            Action::Upload if item.remote.is_some() => (Step::Replace, item.local.as_ref().map_or(0, |m| m.size)),
            Action::Upload => (Step::Upload, item.local.as_ref().map_or(0, |m| m.size)),
            Action::DeleteRemote => (Step::Delete, 0),
            Action::Download if item.local.is_some() => (Step::Overwrite, item.remote.as_ref().map_or(0, |m| m.size)),
            Action::Download => (Step::Download, item.remote.as_ref().map_or(0, |m| m.size)),
        };
        // Downloads don't touch the server: only what goes there or goes away is protected.
        let guarded = item.action != Action::Download && protected.matches(&item.path);
        out.planned.push(Planned { path: item.path.clone(), step, bytes, protected: guarded });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_control::model::{Change, Entry, EnvironmentKind, FileMeta, Mapping};

    fn entry(path: &str, change: Change, local: Option<u64>, remote: Option<u64>) -> Entry {
        Entry {
            path: path.into(),
            local: local.map(|s| FileMeta::new(s, 1)),
            remote: remote.map(|s| FileMeta::new(s, 1)),
            change,
        }
    }

    #[test]
    fn each_file_is_planned_with_its_bytes_and_protection() {
        let env = crate::source_control::model::tests::environment();
        let items = vec![
            TransferItem::from_entry(&entry("Portals/0/new.css", Change::Added, Some(100), None), Action::Upload, false),
            TransferItem::from_entry(&entry("DesktopModules/Shop/View.ascx", Change::Modified, Some(50), Some(40)), Action::Upload, false),
            TransferItem::from_entry(&entry("web.config", Change::Modified, Some(7), Some(6)), Action::Upload, false),
            TransferItem::from_entry(&entry("old.js", Change::Deleted, None, Some(9)), Action::DeleteRemote, false),
            TransferItem::from_entry(&entry("theirs.js", Change::RemoteOnly, None, Some(30)), Action::Download, false),
            TransferItem::from_entry(&entry("c.js", Change::Conflict, Some(1), Some(2)), Action::Upload, false),
        ];
        let p = preview(&env, &items);
        let steps: Vec<(&str, Step, u64, bool)> =
            p.planned.iter().map(|x| (x.path.as_str(), x.step, x.bytes, x.protected)).collect();
        assert_eq!(
            steps,
            [
                ("Portals/0/new.css", Step::Upload, 100, true),
                ("DesktopModules/Shop/View.ascx", Step::Replace, 50, false),
                ("web.config", Step::Replace, 7, true),
                ("old.js", Step::Delete, 0, false),
                ("theirs.js", Step::Download, 30, false),
            ]
        );
        assert_eq!(p.refused, [("c.js".to_string(), "it changed on both sides - choose which one to keep".to_string())]);
        assert_eq!((p.count(Step::Replace), p.bytes(), p.protected().len()), (2, 187, 2));
        assert!(p.destructive());
    }

    #[test]
    fn a_file_no_mapping_holds_is_left_alone() {
        let mut env = crate::source_control::model::tests::environment();
        env.kind = EnvironmentKind::Test;
        env.mappings = vec![Mapping { local: "DesktopModules/Shop".into(), remote: "/srv/shop".into() }];
        let items = vec![TransferItem::from_entry(&entry("web.config", Change::Added, Some(1), None), Action::Upload, false)];
        let p = preview(&env, &items);
        assert!(p.is_empty() && !p.destructive());
        assert_eq!(p.refused[0].1, "no mapping holds it");
    }
}
