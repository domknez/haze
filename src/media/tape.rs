//! Recording a channel in the background, on a connection of its own, whatever
//! plays meanwhile: a programme picked in the guide. Its packets go into a file as
//! they're read, nothing decoded. When the stream drops, it's opened again and the
//! recording carries on in a new file, until its time is up.

use super::record::Recorder;
use super::{Error, Request, demux};
use crate::sync::Lock;
use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The wait before the stream is opened again after it dropped; each next attempt
/// waits twice as long as the one before, up to [`LONGEST_WAIT`].
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// How a background recording goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tape {
    /// The files that hold some of it, the latest last.
    pub files: Vec<PathBuf>,
    /// The stream comes in, into the latest file.
    pub receiving: bool,
    /// Why the stream isn't coming in (it's opened again), or why the recording
    /// ended before its time.
    pub problem: Option<String>,
    /// The recording is over: its time is up, it was stopped, or it can't go on.
    pub over: bool,
}

struct Shared {
    stop: AtomicBool,
    until: Instant,
    tape: Mutex<Tape>,
    repaint: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn done(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || Instant::now() >= self.until
    }

    fn update(&self, change: impl FnOnce(&mut Tape)) {
        change(&mut self.tape.locked());
        (self.repaint)();
    }
}

/// A channel being recorded in the background. Dropping it stops the recording,
/// and waits for its file to be finished.
pub struct Taping {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Taping {
    /// Record `request` until `until`, into the files `next_file` names: one more
    /// each time the stream is opened again.
    pub fn start(
        request: Request,
        until: Instant,
        next_file: impl FnMut() -> PathBuf + Send + 'static,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Taping {
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), until, tape: Mutex::default(), repaint: Box::new(repaint) });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("tape".into()).spawn(move || run(&s, &request, next_file));
        if let Err(e) = &thread {
            shared.update(|t| {
                t.problem = Some(format!("couldn't start: {e}"));
                t.over = true;
            });
        }
        Taping { shared, thread: thread.ok() }
    }

    pub fn tape(&self) -> Tape {
        self.shared.tape.locked().clone()
    }

    /// Stop recording (if it still goes on), finish the file, and say how it went.
    pub fn finish(self) -> Tape {
        let shared = self.shared.clone();
        drop(self);
        shared.tape.locked().clone()
    }
}

impl Drop for Taping {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why recording from one connection stopped before the time was up.
enum Break {
    /// The stream dropped, or couldn't be opened: it's tried again.
    Dropped(String),
    /// The file can't be written: there's no use trying again.
    Failed(String),
}

fn run(shared: &Arc<Shared>, request: &Request, mut next_file: impl FnMut() -> PathBuf) {
    demux::init();
    let mut wait = FIRST_WAIT;
    while !shared.done() {
        let before = shared.tape.locked().files.len();
        let why = match tape(shared, request, &mut next_file) {
            Ok(()) => "the stream ended".to_string(),
            Err(Break::Dropped(why)) => why,
            Err(Break::Failed(why)) => {
                log_warn!("recording: {why}");
                shared.update(|t| t.problem = Some(why));
                break;
            }
        };
        if shared.done() {
            break;
        }
        log_info!("recording: {why}; opening the stream again");
        shared.update(|t| {
            t.receiving = false;
            t.problem = Some(why);
        });
        // A stream that came in is opened again soon; one that doesn't, less often.
        if shared.tape.locked().files.len() > before {
            wait = FIRST_WAIT;
        }
        pause(shared, wait);
        wait = (wait * 2).min(LONGEST_WAIT);
    }
    shared.update(|t| {
        t.receiving = false;
        t.over = true;
    });
}

/// Wait for `wait`, or until the recording is done.
fn pause(shared: &Shared, wait: Duration) {
    let end = Instant::now() + wait;
    while !shared.done() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Open the stream and record it into a new file until it drops or the time is up.
/// A file nothing went into is removed.
fn tape(shared: &Arc<Shared>, request: &Request, next_file: &mut impl FnMut() -> PathBuf) -> Result<(), Break> {
    let dropped = |e: Error| Break::Dropped(e.to_string());
    let s = shared.clone();
    let opened = demux::open_input(request, move || s.done()).map_err(dropped)?;
    let Some(mut input) = opened else { return Ok(()) };
    let video = demux::best_video(&input);
    // The picture, and the sound in every language; nothing else is downloaded.
    let recorded: Vec<usize> =
        input.streams().filter(|s| Some(s.index()) == video || s.parameters().medium() == Type::Audio).map(|s| s.index()).collect();
    if recorded.is_empty() {
        return Err(dropped(Error::Empty));
    }
    for i in 0..input.nb_streams() as usize {
        demux::set_discard(&mut input, i, !recorded.contains(&i));
    }
    let path = next_file();
    let mut recorder =
        Recorder::start(&path, &input, &recorded, video).map_err(|e| Break::Failed(format!("couldn't create the file: {e}")))?;
    let result = pump(shared, &mut input, &mut recorder, &path);
    let empty = recorder.is_empty();
    drop(recorder);
    if empty {
        let _ = std::fs::remove_file(&path);
    }
    result
}

/// Read packets into the file at `path` until the stream ends or drops, or the time
/// is up.
fn pump(shared: &Shared, input: &mut ffmpeg::format::context::Input, recorder: &mut Recorder, path: &Path) -> Result<(), Break> {
    while !shared.done() {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(input) {
            Ok(()) => {
                let first = recorder.is_empty();
                recorder.write(&packet).map_err(|e| Break::Failed(format!("couldn't write to the file: {e}")))?;
                if first && !recorder.is_empty() {
                    shared.update(|t| {
                        t.files.push(path.to_path_buf());
                        t.receiving = true;
                        t.problem = None;
                    });
                }
            }
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
            Err(_) if shared.done() => return Ok(()),
            Err(e) => return Err(Break::Dropped(Error::from(e).to_string())),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seconds of a test picture and a tone, in MPEG-TS.
    const CLIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/media/testdata/clip.ts");

    fn request(url: &str) -> Request {
        Request { url: url.into(), user_agent: None, referrer: None, live: true, start_at: None }
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("haze-tape-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Files numbered 1.mkv, 2.mkv… in `dir`.
    fn numbered(dir: &Path) -> impl FnMut() -> PathBuf + Send + 'static {
        let (dir, mut n) = (dir.to_path_buf(), 0);
        move || {
            n += 1;
            dir.join(format!("{n}.mkv"))
        }
    }

    fn tape(url: &str, dir: &Path, seconds: f64) -> Taping {
        Taping::start(request(url), Instant::now() + Duration::from_secs_f64(seconds), numbered(dir), || {})
    }

    /// Wait until `done` says so, for ten seconds at most.
    fn wait_for(taping: &Taping, done: impl Fn(&Tape) -> bool) -> Tape {
        let start = Instant::now();
        while !done(&taping.tape()) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        taping.tape()
    }

    /// The streams of the file at `path`, and its packets.
    fn contents(path: &Path) -> (Vec<Type>, usize) {
        let mut input = ffmpeg::format::input(path).unwrap();
        let kinds = input.streams().map(|s| s.parameters().medium()).collect();
        (kinds, input.packets().count())
    }

    #[test]
    fn opens_a_stream_that_ended_again_in_a_new_file_until_the_time_is_up() {
        let dir = folder("time");
        let taping = tape(CLIP, &dir, 2.5);
        let tape = wait_for(&taping, |t| t.over);
        assert!(tape.over && !tape.receiving);
        // The clip ends at once, and came in: it's opened again a second later, each time.
        assert!(tape.files.len() >= 2, "{:?}", tape.files);
        assert!(tape.files.iter().zip(1..).all(|(f, n)| *f == dir.join(format!("{n}.mkv"))));
        assert_eq!(tape.problem.as_deref(), Some("the stream ended"));
        for file in &tape.files {
            let (kinds, packets) = contents(file);
            assert_eq!(kinds, [Type::Video, Type::Audio]);
            assert!(packets > 100, "{packets} packets");
        }
        drop(taping);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stops_when_dropped_and_finishes_the_file() {
        let dir = folder("drop");
        let taping = tape(CLIP, &dir, 600.0);
        let tape = wait_for(&taping, |t| !t.files.is_empty());
        let start = Instant::now();
        drop(taping);
        assert!(start.elapsed() < Duration::from_secs(2), "stopped in {:?}", start.elapsed());
        assert!(contents(&tape.files[0]).1 > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_trying_a_stream_that_isnt_there_and_leaves_no_files() {
        let dir = folder("missing");
        let taping = tape(&dir.join("no-such-stream.ts").to_string_lossy(), &dir, 1.5);
        let tape = wait_for(&taping, |t| t.over);
        assert!(tape.over && tape.files.is_empty());
        assert!(tape.problem.is_some());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        drop(taping);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gives_up_when_the_file_cant_be_made() {
        let nowhere = std::env::temp_dir().join("haze-no-such-folder").join("deeper");
        let taping = tape(CLIP, &nowhere, 600.0);
        let tape = wait_for(&taping, |t| t.over);
        assert!(tape.over && tape.files.is_empty());
        assert!(tape.problem.as_deref().is_some_and(|p| p.starts_with("couldn't create the file")), "{:?}", tape.problem);
    }
}
