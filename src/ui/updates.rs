//! Updates in the background: check, download and stage a new release quietly,
//! then offer "Restart to update" in the top bar. A staged update that was never
//! applied installs when haze quits.

use crate::update::{self, Release, Restart, Staged};
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Let startup traffic (the catalog, the guide, logos) go first.
const FIRST_CHECK: Duration = Duration::from_secs(10);

pub enum State {
    UpToDate,
    /// Fetching and unpacking in the background; nothing in the top bar yet.
    Downloading {
        release: Release,
        progress: Arc<AtomicU32>,
        done: Receiver<Result<Staged, String>>,
    },
    /// Staged next to this copy: one click swaps it in and restarts.
    Ready(Release, Staged),
    /// haze can't replace itself here (dev build, read-only folder): offer the page.
    Manual(Release),
    /// Swapping in failed; the button falls back to the release page.
    Failed(Release, String),
}

pub struct Updates {
    /// "Check automatically" (setting).
    pub auto: bool,
    pub state: State,
    checking: Option<Receiver<Result<Option<Release>, String>>>,
    /// A check has completed since startup.
    checked: bool,
    next_check: Instant,
    /// Why the last check or download failed, for the settings menu.
    last_error: Option<String>,
}

impl Updates {
    pub fn new(auto: bool) -> Self {
        Self { auto, state: State::UpToDate, checking: None, checked: false, next_check: Instant::now() + FIRST_CHECK, last_error: None }
    }

    /// One line for the settings menu.
    pub fn status(&self) -> String {
        let current = update::CURRENT;
        match &self.state {
            _ if self.checking.is_some() => "Checking…".into(),
            State::Downloading { release, progress, .. } => {
                format!("Downloading haze {}… {}%", release.version, progress.load(Ordering::Relaxed) / 10)
            }
            State::Ready(release, _) => format!("haze {} is ready to install", release.version),
            State::Manual(release) | State::Failed(release, _) => format!("haze {} is available", release.version),
            State::UpToDate => match &self.last_error {
                Some(_) => "Couldn't check for updates".into(),
                None if self.checked => format!("haze {current} is the latest version"),
                None => format!("You have haze {current}"),
            },
        }
    }

    /// What the top bar offers, if anything: its label, and whether it installs
    /// (rather than opening the download page).
    pub fn offer(&self) -> Option<(String, bool)> {
        match &self.state {
            State::Ready(release, _) => Some((format!("Restart to update to {}", release.version), true)),
            State::Manual(release) => Some((format!("haze {} is out", release.version), false)),
            State::Failed(..) => Some(("Update failed".into(), false)),
            State::UpToDate | State::Downloading { .. } => None,
        }
    }

    /// Why installing failed, for the top bar's hint.
    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            State::Failed(_, e) => Some(e),
            _ => None,
        }
    }

    /// Start a check now (menu "Check now", or when one is due).
    pub fn check_now(&mut self, ctx: &egui::Context) {
        let busy = matches!(self.state, State::Downloading { .. } | State::Ready(..));
        if self.checking.is_some() || busy {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check());
            ctx.request_repaint();
        });
        self.checking = Some(rx);
        self.next_check = Instant::now() + INTERVAL;
    }

    /// Call every frame: runs due checks and collects background results.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.auto && Instant::now() >= self.next_check {
            self.check_now(ctx);
        }
        if self.auto {
            ctx.request_repaint_after(self.next_check.saturating_duration_since(Instant::now()));
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(result) => {
                    self.checking = None;
                    self.checked = true;
                    match result {
                        Ok(Some(release)) => self.found(ctx, release),
                        Ok(None) => self.last_error = None,
                        Err(e) => {
                            log_debug!("update check failed: {e}");
                            self.last_error = Some(e);
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.checking = None,
            }
        }
        if let State::Downloading { done, release, .. } = &self.state {
            ctx.request_repaint_after(Duration::from_millis(500));
            match done.try_recv() {
                Ok(Ok(staged)) => {
                    log_info!("update: {} staged, waiting for a restart", staged.version);
                    self.state = State::Ready(release.clone(), staged);
                }
                // Quietly try again at the next check.
                Ok(Err(e)) => {
                    log_warn!("update download failed: {e}");
                    self.last_error = Some(e);
                    self.state = State::UpToDate;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.state = State::UpToDate,
            }
        }
    }

    /// A newer release exists: download it in the background, or offer its page.
    fn found(&mut self, ctx: &egui::Context, release: Release) {
        self.last_error = None;
        if !update::can_install(&release) {
            log_info!("update: {} available (install manually)", release.version);
            self.state = State::Manual(release);
            return;
        }
        log_info!("update: {} available, downloading", release.version);
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, done) = std::sync::mpsc::channel();
        let (p, ctx, pending) = (progress.clone(), ctx.clone(), release.clone());
        std::thread::spawn(move || {
            let _ = tx.send(update::stage(&pending, &p));
            ctx.request_repaint();
        });
        self.state = State::Downloading { release, progress, done };
    }

    /// The top bar's button was clicked: swap the update in and restart (the caller
    /// relaunches and quits), or open the download page.
    pub fn act(&mut self, ctx: &egui::Context) -> Option<Restart> {
        match std::mem::replace(&mut self.state, State::UpToDate) {
            State::Ready(release, staged) => match update::apply(&staged) {
                Ok(restart) => return Some(restart),
                Err(e) => {
                    log_warn!("update failed: {e}");
                    self.state = State::Failed(release, e);
                }
            },
            State::Manual(release) | State::Failed(release, _) => {
                ctx.open_url(egui::OpenUrl::new_tab(&release.page));
                self.state = State::Manual(release);
            }
            other => self.state = other,
        }
        None
    }

    /// haze is quitting: install a staged update so the next start is the new version.
    pub fn apply_on_exit(&mut self) {
        if let State::Ready(_, staged) = &self.state {
            match update::apply(staged) {
                Ok(_) => log_info!("update: installed on quit"),
                Err(e) => log_warn!("update on quit failed: {e}"),
            }
        }
    }
}
