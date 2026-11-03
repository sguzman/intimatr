//! Explicit offline single-player Windows game attachment helper.
//! Launched only after native Bottles CLI validates the exact game selection.
#[cfg(windows)]
#[path = "intimatr_attach/helper.rs"]
mod windows_attach;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_attach::helper::run_selected_game() {
        eprintln!("Wine game attach helper: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-game-attach-helper must be built for Windows and launched under Wine");
    std::process::exit(2);
}
