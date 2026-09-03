//! Filesystem side effects triggered from the UI.

use std::path::Path;

pub fn open_path(path: &Path) -> Result<(), String> {
    open::that(path).map_err(|e| format!("Could not open {}: {e}", path.display()))
}

pub fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open Explorer: {e}"))
    }
    #[cfg(not(windows))]
    {
        let parent = path.parent().unwrap_or(path);
        open_path(parent)
    }
}

pub fn delete_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| format!("Could not move {} to the Recycle Bin: {e}", path.display()))
}
