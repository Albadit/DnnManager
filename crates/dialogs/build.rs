//! Embeds the release notes (`.docs/release-notes/v*.md`) for What's new, as the WPF app's project file does
//! (`<EmbeddedResource Include=".docs\release-notes\v*.md" />`): every file there, without a list to keep up to date.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let folder = manifest.join("../../.docs/release-notes");
    println!("cargo:rerun-if-changed={}", folder.display());

    let mut notes: Vec<(String, PathBuf)> = std::fs::read_dir(&folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter_map(|p| {
                    let name = p.file_name()?.to_str()?.to_string();
                    let tag = name.strip_suffix(".md").or_else(|| name.strip_suffix(".MD"))?;
                    tag.starts_with('v').then(|| (tag.to_string(), p.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    notes.sort();

    let mut code = String::from("&[\n");
    for (tag, path) in &notes {
        let _ = writeln!(code, "    ({tag:?}, include_str!({:?})),", path.display().to_string());
    }
    code.push_str("]\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("release_notes.rs");
    std::fs::write(out, code).expect("write release_notes.rs");
}
