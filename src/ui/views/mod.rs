//! One module per part of the screen, each an `impl App` block.

pub mod add_source;
mod bottom;
mod guide;
pub mod pin;
mod popups;
mod rail;
mod settings;
mod stage;
mod top_bar;

pub use bottom::{duration_text, hhmm};
pub use guide::GuideSearch;
pub use settings::Page as SettingsPage;
