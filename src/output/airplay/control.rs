//! The session's control requests (pause, resume, flush, volume) run in order on a
//! worker thread of their own, so the player never waits for a speaker's RTSP
//! replies (up to a couple of seconds when the network is bad). The sink only
//! queues them; [`run`] carries them out.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

/// A control request, carried out in the order asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Pause,
    Resume,
    /// Drop what is buffered, here and on the receiver.
    Flush,
    /// 0.0 ..= 1.0
    Volume(f32),
}

#[derive(Default)]
struct State {
    ops: VecDeque<Op>,
    closed: bool,
}

/// Requests waiting for the worker.
#[derive(Default)]
pub struct Queue {
    state: Mutex<State>,
    wake: Condvar,
    /// Flushes asked for and not done yet. Audio written meanwhile would be thrown
    /// away with the old, so the sink takes none until they are.
    flushes: AtomicUsize,
}

impl Queue {
    /// Queue `op`. A volume right after another volume replaces it: only the latest
    /// level matters (a fade sends one twice a second).
    pub fn push(&self, op: Op) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        if let Op::Volume(volume) = op
            && let Some(Op::Volume(last)) = state.ops.back_mut()
        {
            *last = volume;
            return;
        }
        if op == Op::Flush {
            self.flushes.fetch_add(1, Ordering::AcqRel);
        }
        state.ops.push_back(op);
        self.wake.notify_one();
    }

    /// A flush is queued or under way.
    pub fn flushing(&self) -> bool {
        self.flushes.load(Ordering::Acquire) > 0
    }

    /// Stop: drop what is still queued and let [`run`] return.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        let dropped = state.ops.drain(..).filter(|op| *op == Op::Flush).count();
        self.flushes.fetch_sub(dropped, Ordering::AcqRel);
        self.wake.notify_one();
    }

    /// The next request, waiting for one; None once closed.
    fn next(&self) -> Option<Op> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(op) = state.ops.pop_front() {
                return Some(op);
            }
            state = self.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Carry out the queued requests with `apply`, in order, until the queue is closed.
/// A flush counts as done (and the sink takes audio again) only once `apply` has
/// returned from it.
pub fn run(queue: &Queue, mut apply: impl FnMut(Op)) {
    while let Some(op) = queue.next() {
        apply(op);
        if op == Op::Flush {
            queue.flushes.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Everything queued so far, without a worker.
    fn queued(queue: &Queue) -> Vec<Op> {
        queue.lock().ops.iter().copied().collect()
    }

    #[test]
    fn consecutive_volumes_keep_the_latest() {
        let queue = Queue::default();
        queue.push(Op::Volume(0.5));
        queue.push(Op::Volume(0.4));
        queue.push(Op::Volume(0.3));
        queue.push(Op::Pause);
        queue.push(Op::Volume(0.2));
        queue.push(Op::Resume);
        queue.push(Op::Volume(0.1));
        queue.push(Op::Volume(0.0));
        assert_eq!(queued(&queue), [Op::Volume(0.3), Op::Pause, Op::Volume(0.2), Op::Resume, Op::Volume(0.0)]);
    }

    #[test]
    fn flushing_lasts_until_the_flush_is_done() {
        let queue = Arc::new(Queue::default());
        let (started_tx, started) = mpsc::channel();
        let (finish_tx, finish) = mpsc::channel::<()>();
        let worker = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                let mut done = Vec::new();
                run(&queue, |op| {
                    if op == Op::Flush {
                        started_tx.send(()).unwrap();
                        finish.recv().unwrap(); // a slow FLUSH round trip
                    }
                    done.push(op);
                });
                done
            })
        };
        assert!(!queue.flushing());
        queue.push(Op::Pause);
        queue.push(Op::Flush);
        assert!(queue.flushing(), "from the moment it is asked for");
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(queue.flushing(), "while the worker is at it");
        queue.push(Op::Resume);
        finish_tx.send(()).unwrap();
        while queue.flushing() {
            std::thread::yield_now();
        }
        queue.push(Op::Volume(1.0));
        // Once the worker has taken everything, stop it (what it took still runs).
        while !queue.lock().ops.is_empty() {
            std::thread::yield_now();
        }
        queue.close();
        assert_eq!(worker.join().unwrap(), [Op::Pause, Op::Flush, Op::Resume, Op::Volume(1.0)], "in the order asked");
    }

    #[test]
    fn closing_drops_what_is_left() {
        let queue = Queue::default();
        queue.push(Op::Flush);
        queue.push(Op::Volume(0.5));
        queue.close();
        assert!(!queue.flushing(), "a dropped flush doesn't hold writes back");
        queue.push(Op::Pause);
        let mut ran = Vec::new();
        run(&queue, |op| ran.push(op));
        assert!(ran.is_empty());
    }
}
