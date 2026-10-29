//! Background loads for the UI: run a closure on a thread, collect its result on a
//! later frame (the thread wakes the UI when it finishes) and keep it, or why it
//! failed.

use eframe::egui;
use std::sync::mpsc::{Receiver, TryRecvError};

/// One resource loaded in the background. Starting it again, or replacing the
/// `Load`, drops the receiver: a result still on its way is never applied.
#[derive(Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading(Receiver<Result<T, String>>),
    Ready(T),
    /// Why it failed, worded for the viewer.
    Failed(String),
}

impl<T: Send + 'static> Load<T> {
    pub fn start(ctx: &egui::Context, work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Load<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        Load::Loading(rx)
    }

    /// Takes the result if it arrived; true when it did.
    pub fn poll(&mut self) -> bool {
        let Load::Loading(rx) = self else { return false };
        match rx.try_recv() {
            Ok(Ok(v)) => *self = Load::Ready(v),
            Ok(Err(e)) => *self = Load::Failed(e),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => *self = Load::Failed("the load stopped unexpectedly".into()),
        }
        true
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Load::Loading(_))
    }
}
