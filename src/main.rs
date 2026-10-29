#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(clippy::chunks_exact_to_as_chunks)]

//! haze — a free and open source, lightweight IPTV player with AirPlay output.

#[macro_use]
mod log;
mod awake;
mod cli;
mod credentials;
mod icon;
mod iptv;
mod library;
mod media;
mod output;
mod settings;
mod ui;

fn main() -> eframe::Result {
    log::install_panic_hook();
    match cli::Command::parse(std::env::args().skip(1)) {
        Some(command) => {
            command.run();
            Ok(())
        }
        None => ui::run(),
    }
}
