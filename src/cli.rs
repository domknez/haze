//! Command-line tools for looking into a stream or a source without the window.

use crate::media::{Player, Request, Status};
use crate::output::Output;
use std::time::{Duration, Instant};

pub enum Command {
    /// Play a stream for a few seconds and report what it is.
    Probe {
        url: String,
        seconds: u64,
    },
    /// Is a newer release out, and can this copy install it.
    CheckUpdate,
    /// Install the latest release in place (no relaunch).
    SelfUpdate,
    Help,
}

impl Command {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Option<Command> {
        let first = args.next()?;
        Some(match first.as_str() {
            "probe" => match args.next() {
                Some(url) => Command::Probe { url, seconds: args.next().and_then(|s| s.parse().ok()).unwrap_or(8) },
                None => Command::Help,
            },
            "check-update" => Command::CheckUpdate,
            "self-update" => Command::SelfUpdate,
            _ => Command::Help,
        })
    }

    pub fn run(self) {
        match self {
            Command::Probe { url, seconds } => probe(&url, seconds),
            Command::CheckUpdate => check_update(),
            Command::SelfUpdate => self_update(),
            Command::Help => println!("usage: haze [probe <url> [seconds] | check-update | self-update]"),
        }
    }
}

fn check_update() {
    match crate::update::check() {
        Ok(Some(release)) => {
            println!("haze {} is available (you have {})", release.version, crate::update::CURRENT);
            println!("installs in place: {}", crate::update::can_install(&release));
        }
        Ok(None) => println!("haze {} is the latest version", crate::update::CURRENT),
        Err(e) => println!("couldn't check for updates: {e}"),
    }
}

fn self_update() {
    let release = match crate::update::check() {
        Ok(Some(release)) => release,
        Ok(None) => return println!("haze {} is the latest version", crate::update::CURRENT),
        Err(e) => return println!("couldn't check for updates: {e}"),
    };
    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    match crate::update::install(&release, &progress) {
        Ok(_) => println!("installed haze {}", release.version),
        Err(e) => println!("update failed: {e}"),
    }
}

fn probe(url: &str, seconds: u64) {
    // A file on disk is a recording; anything else is taken for a live channel.
    let live = !std::path::Path::new(url).is_file();
    let request = Request { url: url.to_string(), user_agent: None, referrer: None, live };
    let player = Player::open(request, Output::Local, 0.3, || {});
    let start = Instant::now();
    let mut last = (0, Status::Opening);
    let (mut subtitles_on, mut said) = (false, None);
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(250));
        // The first subtitle track with text, shown as it goes.
        if !subtitles_on && let Some(track) = player.info().subtitle_tracks.first() {
            println!("subtitles: {}", track.label);
            player.set_subtitle_track(Some(track.stream));
            subtitles_on = true;
        }
        let line = player.subtitles();
        if line.is_some() && line != said {
            println!(
                "{:5.1}s subtitle at {:.2}: {}",
                start.elapsed().as_secs_f32(),
                player.position().unwrap_or(0.0),
                line.as_deref().unwrap_or("").replace('\n', " / ")
            );
        }
        said = line;
        let (serial, frame) = player.frame();
        let status = player.status();
        if status != last.1 || serial != last.0 {
            let size = frame.map(|f| format!("{}x{} {:?} pts {:.2}", f.width, f.height, f.layout, f.pts)).unwrap_or_default();
            println!("{:5.1}s {status:?} frames {serial} {size} clock {:?}", start.elapsed().as_secs_f32(), player.position());
            last = (serial, status.clone());
        }
        if let Status::Failed(_) | Status::Ended = status {
            break;
        }
        if let Some(n) = player.take_notice() {
            println!("notice: {n}");
        }
    }
    println!("{:#?}", player.info());
}
