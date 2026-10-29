#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(clippy::chunks_exact_to_as_chunks)]

//! haze — a free and open source, lightweight IPTV player with AirPlay output.

#[macro_use]
mod log;
mod awake;
mod iptv;
mod output;

fn main() {
    log::install_panic_hook();
}
