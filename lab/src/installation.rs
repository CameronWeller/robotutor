use std::{env, fs, path::PathBuf};

/// Resolve the source-backed workers and course assets for native or staged builds.
pub fn project_root(source_checkout: PathBuf) -> PathBuf {
    env::var_os("ROBOTUTOR_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            let marker = env::current_exe().ok()?.with_file_name("project-root.txt");
            let root = fs::read_to_string(marker).ok()?;
            Some(PathBuf::from(root.trim()))
        })
        .unwrap_or(source_checkout)
}
