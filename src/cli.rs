//! Command-line tools for looking into a stream or a source without the window.

use crate::media::{Player, Request, Status};
use crate::output::Output;
use std::time::{Duration, Instant};

pub enum Command {
    /// Play a stream for a few seconds and report what it is.
    Probe { url: String, seconds: u64 },
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
            _ => Command::Help,
        })
    }

    pub fn run(self) {
        match self {
            Command::Probe { url, seconds } => probe(&url, seconds),
            Command::Help => println!("usage: haze [probe <url> [seconds]]"),
        }
    }
}

fn probe(url: &str, seconds: u64) {
    let request = Request { url: url.to_string(), user_agent: None, referrer: None, live: true };
    let player = Player::open(request, Output::Local, 0.3, || {});
    let start = Instant::now();
    let mut last = (0, Status::Opening);
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(500));
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
