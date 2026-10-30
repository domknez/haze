//! egui front-end in the "Cinema" look: the picture fills the window and the
//! controls float over it as glass panels, fading away while you watch.
//!
//! - [`app`]: state, what happens each frame, and what actions do; [`browse`]:
//!   what the rail lists; [`session`]: what plays
//! - [`views`]: the top bar, channel rail, bottom panel, menus, dialogs
//! - [`video`]: the picture, drawn on the GPU from the decoder's YUV planes
//! - [`style`], [`icons`], [`widgets`]: the look
//! - [`logos`]: channel logos and posters; [`load`]: background loads
//! - [`now_playing`]: media keys and the OS "Now Playing" widget
//! - [`updates`]: new releases, downloaded in the background

mod app;
mod browse;
mod icons;
mod load;
mod logos;
mod now_playing;
mod session;
mod style;
mod updates;
mod video;
mod views;
mod widgets;

use eframe::egui;
use style::metrics;

/// Open the main window and run the app until it closes.
pub fn run() -> eframe::Result {
    crate::log::to_file();
    log_info!("haze {} starting ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    crate::update::clean_up();
    let viewport = egui::ViewportBuilder::default()
        .with_title("haze")
        .with_inner_size(metrics::WINDOW_SIZE)
        .with_min_inner_size(metrics::MIN_WINDOW_SIZE)
        .with_drag_and_drop(true)
        .with_icon(std::sync::Arc::new(crate::icon::window_icon()));
    // The picture under a transparent title bar, traffic lights floating over it.
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_fullsize_content_view(true).with_title_shown(false).with_titlebar_shown(false);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("haze", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
