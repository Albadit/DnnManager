//! A reporter and a prompt for the tests.

use std::sync::Mutex;

use dnnm_application::operation::{Prompt, Reporter};

/// Every line reported, the kind as a prefix.
#[derive(Default)]
pub struct Lines(pub Vec<String>);

impl Reporter for Lines {
    fn step(&mut self, t: &str) {
        self.0.push(format!("## {t}"));
    }
    fn info(&mut self, t: &str) {
        self.0.push(t.into());
    }
    fn success(&mut self, t: &str) {
        self.0.push(format!("ok {t}"));
    }
    fn warn(&mut self, t: &str) {
        self.0.push(format!("warn {t}"));
    }
    fn fail(&mut self, t: &str) {
        self.0.push(format!("! {t}"));
    }
    fn fact(&mut self, name: &str, value: &str) {
        self.0.push(format!("fact {name}: {value}"));
    }
}

/// Answers every question the same way and records them.
pub struct Answer(pub bool, pub Mutex<Vec<String>>);

impl Answer {
    pub fn no() -> Self {
        Self(false, Mutex::new(vec![]))
    }
}

impl Prompt for Answer {
    fn confirm(&self, q: &str, _: &str, _: &str, _: bool) -> bool {
        self.1.lock().unwrap().push(q.into());
        self.0
    }
    fn choose(&self, q: &str, _: &[&str]) -> Option<usize> {
        self.1.lock().unwrap().push(q.into());
        None
    }
}
