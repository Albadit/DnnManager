//! Off Windows: no editor, IDE or SQL Server Management Studio is looked for - the editors and SSMS this menu starts
//! are the Windows programs, started next to IIS. "Open with…" is shown greyed with [`crate::NOTHING_FOUND`].

/// What is installed (`locate.rs`): nothing found here.
pub mod locate {
    use std::path::{Path, PathBuf};

    /// A code editor, an IDE or SQL Server Management Studio.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Ide {
        pub name: String,
        pub exe_path: PathBuf,
        /// Opens a project's `.sln` rather than its folder when there is exactly one.
        pub opens_solution: bool,
        /// The product's major version, or 0 when unknown.
        pub major_version: u32,
    }

    pub fn installed() -> &'static [Ide] {
        &[]
    }

    pub fn management_studios() -> &'static [Ide] {
        &[]
    }

    pub fn refusal(_ssms: &Ide) -> Option<String> {
        None
    }

    pub fn solution_for(_project_directory: &Path) -> Option<PathBuf> {
        None
    }
}

/// The password left on the clipboard for SSMS: never left off Windows.
pub mod secret_clipboard {
    pub fn clear_if_still_ours() {}
}
