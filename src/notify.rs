use std::sync::Arc;
use std::sync::atomic::{AtomicInt, Relaxed};
use error::MioResult;
use io::IoHandle;
use os;
use util::BoundedQueue;

const SLEEP: int = -1;

/// Send notifications to the event loop, waking it up if necessary. If the
/// event loop is not currently sleeping, avoid using an OS wake-up strategy
/// (eventfd, pipe, ...). Backed by a pre-allocated lock free MPMC queue.
///
/// TODO: Use more efficient wake-up strategy if available
pub struct Notify<M: Send> {
    inner: Arc<NotifyInner<M>>
}

impl<M: Send> Notify<M> {
    #[inline]
    pub fn with_capacity(capacity: uint) -> MioResult<Notify<M>> {
        Ok(Notify {
            inner: Arc::new(try!(NotifyInner::with_capacity(capacity)))
        })
    }

    #[inline]
    pub fn check(&self, max: uint, will_sleep: bool) -> uint {
        self.inner.check(max, will_sleep)
    }

    #[inline]
    pub fn notify(&self, value: M) -> Result<(), M> {
        self.inner.notify(value)
    }

    #[inline]
    pub fn poll(&self) -> Option<M> {
        self.inner.poll()
    }

    #[inline]
    pub fn cleanup(&self) {
        self.inner.cleanup();
    }
}

impl<M: Send> Clone for Notify<M> {
    fn clone(&self) -> Notify<M> {
        Notify {
            inner: self.inner.clone()
        }
    }
}

struct NotifyInner<M> {
    state: AtomicInt,
    queue: BoundedQueue<M>,
    awaken: os::Awakener
}

impl<M: Send> NotifyInner<M> {
    fn with_capacity(capacity: uint) -> MioResult<NotifyInner<M>> {
        Ok(NotifyInner {
            state: AtomicInt::new(0),
            queue: BoundedQueue::with_capacity(capacity),
            awaken: try!(os::Awakener::new())
        })
    }

    fn check(&self, max: uint, will_sleep: bool) -> uint {
        let max = max as int;
        let mut cur = self.state.load(Relaxed);
        let mut nxt;
        let mut val;

        loop {
            // If there are pending messages, then whether or not the event loop
            // was planning to sleep does not matter - it will not sleep.
            if cur > 0 {
                if max >= cur {
                    nxt = 0;
                } else {
                    nxt = cur - max;
                }
            } else {
                if will_sleep {
                    nxt = SLEEP;
                } else {
                    nxt = 0;
                }
            }

            val = self.state.compare_and_swap(cur, nxt, Relaxed);

            if val == cur {
                break;
            }

            cur = val;
        }

        if cur < 0 {
            0
        } else {
            cur as uint
        }
    }

    fn poll(&self) -> Option<M> {
        self.queue.pop()
    }

    fn notify(&self, value: M) -> Result<(), M> {
        // First, push the message onto the queue
        if !self.queue.push(value) {
            // TODO: Don't fail
            panic!("queue full");
        }

        let mut cur = self.state.load(Relaxed);
        let mut nxt;
        let mut val;

        loop {
            nxt = if cur == SLEEP { 1 } else { cur + 1 };
            val = self.state.compare_and_swap(cur, nxt, Relaxed);

            if val == cur {
                break;
            }

            cur = val;
        }

        if cur == SLEEP {
            if self.awaken.wakeup().is_err() {
                // TODO: Don't fail
                panic!("failed to awaken event loop");
            }
        }

        Ok(())
    }

    fn cleanup(&self) {
        self.awaken.cleanup();
    }
}

impl<M: Send> IoHandle for Notify<M> {
    fn desc(&self) -> &os::IoDesc {
        self.inner.awaken.desc()
    }
}
