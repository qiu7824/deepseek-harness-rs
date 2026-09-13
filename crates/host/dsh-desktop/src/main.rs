#![cfg_attr(windows, windows_subsystem = "windows")]
mod assets;
mod host;
mod markdown;
mod model;
#[cfg(test)]
mod tests;
mod theme;
mod ui;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("dsh-desktop {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    ui::run()
}
