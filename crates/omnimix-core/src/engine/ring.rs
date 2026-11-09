//! Lock-free single-producer / single-consumer ring of interleaved f32 frames.
//! The producer is a platform capture thread, the consumer is a bus render thread.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    ch: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    overruns: AtomicUsize,
}

// SAFETY: head/tail ordering guarantees the producer and consumer never touch
// the same slot concurrently (classic SPSC protocol).
unsafe impl Sync for Inner {}
unsafe impl Send for Inner {}

pub struct Producer {
    inner: Arc<Inner>,
}

pub struct Consumer {
    inner: Arc<Inner>,
}

/// Create a ring holding at least `capacity_frames` frames of `channels` samples.
pub fn ring(capacity_frames: usize, channels: usize) -> (Producer, Consumer) {
    let ch = channels.max(1);
    let cap = (capacity_frames.max(16) * ch).next_power_of_two();
    let buf: Vec<UnsafeCell<f32>> = (0..cap).map(|_| UnsafeCell::new(0.0)).collect();
    let inner = Arc::new(Inner {
        buf: buf.into_boxed_slice(),
        mask: cap - 1,
        ch,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        overruns: AtomicUsize::new(0),
    });
    (Producer { inner: inner.clone() }, Consumer { inner })
}

impl Inner {
    fn capacity_frames(&self) -> usize {
        (self.mask + 1) / self.ch
    }
}

impl Producer {
    pub fn channels(&self) -> usize {
        self.inner.ch
    }

    pub fn free_frames(&self) -> usize {
        let i = &self.inner;
        let used = i.head.load(Ordering::Relaxed).wrapping_sub(i.tail.load(Ordering::Acquire));
        (i.mask + 1 - used) / i.ch
    }

    /// Push interleaved frames. Returns frames written (drops the rest on overrun).
    pub fn push(&mut self, data: &[f32]) -> usize {
        let frames = data.len() / self.inner.ch;
        self.push_with(frames, |i| data[i])
    }

    pub fn push_silence(&mut self, frames: usize) -> usize {
        self.push_with(frames, |_| 0.0)
    }

    /// Push `frames` frames where sample `i` (interleaved index) is `f(i)`.
    pub fn push_with<F: FnMut(usize) -> f32>(&mut self, frames: usize, mut f: F) -> usize {
        let i = &self.inner;
        let free = self.free_frames();
        let n = frames.min(free);
        if n < frames {
            i.overruns.fetch_add(1, Ordering::Relaxed);
        }
        let head = i.head.load(Ordering::Relaxed);
        for s in 0..n * i.ch {
            let slot = &i.buf[head.wrapping_add(s) & i.mask];
            // SAFETY: slots between head and tail+cap belong to the producer.
            unsafe { *slot.get() = f(s) };
        }
        i.head.store(head.wrapping_add(n * i.ch), Ordering::Release);
        n
    }

    pub fn overruns(&self) -> usize {
        self.inner.overruns.load(Ordering::Relaxed)
    }
}

impl Consumer {
    pub fn channels(&self) -> usize {
        self.inner.ch
    }

    pub fn capacity_frames(&self) -> usize {
        self.inner.capacity_frames()
    }

    pub fn available(&self) -> usize {
        let i = &self.inner;
        i.head.load(Ordering::Acquire).wrapping_sub(i.tail.load(Ordering::Relaxed)) / i.ch
    }

    /// Append up to `frames` frames to `dst`; zero-fills the remainder. Returns frames actually read.
    pub fn pop_into(&mut self, dst: &mut Vec<f32>, frames: usize) -> usize {
        let i = &self.inner;
        let n = frames.min(self.available());
        let tail = i.tail.load(Ordering::Relaxed);
        for s in 0..n * i.ch {
            // SAFETY: slots between tail and head belong to the consumer.
            dst.push(unsafe { *i.buf[tail.wrapping_add(s) & i.mask].get() });
        }
        i.tail.store(tail.wrapping_add(n * i.ch), Ordering::Release);
        for _ in n * i.ch..frames * i.ch {
            dst.push(0.0);
        }
        n
    }

    pub fn skip(&mut self, frames: usize) {
        let i = &self.inner;
        let n = frames.min(self.available());
        let tail = i.tail.load(Ordering::Relaxed);
        i.tail.store(tail.wrapping_add(n * i.ch), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spsc_threads_preserve_order() {
        let (mut p, mut c) = ring(256, 2);
        let t = std::thread::spawn(move || {
            let mut next = 0u32;
            while next < 100_000 {
                let chunk: Vec<f32> = (0..64).flat_map(|k| {
                    let v = (next + k) as f32;
                    [v, -v]
                }).collect();
                let w = p.push(&chunk);
                next += w as u32;
                if w < 64 {
                    std::thread::yield_now();
                }
            }
        });
        let mut expect = 0u32;
        let mut buf = Vec::with_capacity(512);
        while expect < 100_000 {
            buf.clear();
            let n = c.pop_into(&mut buf, 32.min(c.available()));
            for f in 0..n {
                assert_eq!(buf[f * 2], expect as f32);
                assert_eq!(buf[f * 2 + 1], -(expect as f32));
                expect += 1;
            }
            if n == 0 {
                std::thread::yield_now();
            }
        }
        t.join().unwrap();
    }

    #[test]
    fn underflow_zero_fills() {
        let (mut p, mut c) = ring(64, 1);
        p.push(&[1.0, 2.0]);
        let mut v = vec![];
        assert_eq!(c.pop_into(&mut v, 4), 2);
        assert_eq!(v, vec![1.0, 2.0, 0.0, 0.0]);
    }
}
