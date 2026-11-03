//! Strictly fixture-only Wine remote loader entrypoint.
#[cfg(windows)]
#[path = "intimatr_attach/helper.rs"]
mod windows_attach;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_attach::helper::run_fixture() {
        eprintln!("Wine attach helper: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-attach-helper must be built for Windows and launched under Wine");
    std::process::exit(2);
}
