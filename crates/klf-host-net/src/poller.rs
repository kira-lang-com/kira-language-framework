//! Pollers: one wait for readiness on many sockets, finished lookups and
//! wakes from other threads.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mio::event::Source;
use mio::{Events, Interest, Poll, Registry, Token, Waker};

use crate::table::{self, Resource};
use crate::{EVENT_WORDS, Failure, bits, length, word};

// The token mio reports the waker under; a program's own tokens are
// non-negative words, which never reach it.
const WAKE: Token = Token(usize::MAX);

/// What a lookup's worker thread and `wake` reach without the poll lock,
/// which a waiting thread holds.
pub(crate) struct Signals {
    waker: Waker,
    resolved: Mutex<Vec<i64>>,
    woken: AtomicBool,
}

impl Signals {
    pub(crate) fn resolved(&self, token: i64) {
        table::lock(&self.resolved).push(token);
        // A failed wake leaves the token queued for the next wait to find.
        let _ = self.waker.wake();
    }
}

pub(crate) struct Poller {
    poll: Mutex<Poll>,
    registry: Registry,
    pub(crate) signals: Arc<Signals>,
}

/// Makes a poller and answers its handle.
#[must_use]
pub fn create() -> i64 {
    word((|| {
        let poll = Poll::new()?;
        let registry = poll.registry().try_clone()?;
        let waker = Waker::new(poll.registry(), WAKE)?;
        let signals = Arc::new(Signals {
            waker,
            resolved: Mutex::new(Vec::new()),
            woken: AtomicBool::new(false),
        });
        let poller = Poller {
            poll: Mutex::new(poll),
            registry,
            signals,
        };
        Ok(table::insert(Resource::Poller(poller)))
    })())
}

pub(crate) fn poller(handle: i64) -> Result<Arc<Resource>, Failure> {
    let resource = table::get(handle)?;
    if matches!(&*resource, Resource::Poller(_)) {
        Ok(resource)
    } else {
        Err(Failure::InvalidHandle)
    }
}

fn interest(word: i64) -> Option<Interest> {
    let readable = word & bits::READABLE != 0;
    let writable = word & bits::WRITABLE != 0;
    match (readable, writable) {
        (true, true) => Some(Interest::READABLE | Interest::WRITABLE),
        (true, false) => Some(Interest::READABLE),
        (false, true) => Some(Interest::WRITABLE),
        (false, false) => None,
    }
}

fn register(
    registry: &Registry,
    source: &mut dyn Source,
    token: Token,
    wanted: Option<Interest>,
) -> io::Result<()> {
    let Some(wanted) = wanted else {
        return registry.deregister(source);
    };
    match registry.register(source, token, wanted) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            registry.reregister(source, token, wanted)
        }
        result => result,
    }
}

/// Watches a socket for `interest` (readable 1, writable 2) under `token`,
/// replacing what it was watched for before; an interest of 0 stops watching.
#[must_use]
pub fn watch(handle: i64, socket: i64, interest_word: i64, token: i64) -> i64 {
    word((|| {
        let token = usize::try_from(token)
            .ok()
            .filter(|token| Token(*token) != WAKE)
            .ok_or(Failure::InvalidHandle)?;
        let resource = poller(handle)?;
        let Resource::Poller(poller) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let watched = table::get(socket)?;
        let wanted = interest(interest_word);
        let registry = &poller.registry;
        match &*watched {
            Resource::Udp(socket) => {
                register(registry, &mut *table::lock(socket), Token(token), wanted)
            }
            Resource::Listener(listener) => {
                register(registry, &mut *table::lock(listener), Token(token), wanted)
            }
            Resource::Stream(stream) => {
                register(registry, &mut *table::lock(stream), Token(token), wanted)
            }
            Resource::Poller(_) | Resource::Lookup(_) => return Err(Failure::InvalidHandle),
        }?;
        Ok(1)
    })())
}

fn event_bits(event: &mio::event::Event) -> i64 {
    let mut word = 0;
    if event.is_readable() {
        word |= bits::READABLE;
    }
    if event.is_writable() {
        word |= bits::WRITABLE;
    }
    if event.is_read_closed() || event.is_write_closed() {
        word |= bits::PEER_CLOSED;
    }
    if event.is_error() {
        word |= bits::ERROR;
    }
    word
}

/// Collects events into `destination`, two words each, as they fit.
struct Collected<'a> {
    destination: &'a mut [i64],
    count: usize,
}

impl Collected<'_> {
    fn room(&self) -> usize {
        self.destination.len() / EVENT_WORDS - self.count
    }

    fn push(&mut self, token: i64, bits: i64) {
        let at = self.count * EVENT_WORDS;
        self.destination[at] = token;
        self.destination[at + 1] = bits;
        self.count += 1;
    }

    // Lookups that finished and a wake, ahead of socket readiness. What does
    // not fit stays queued for the next wait.
    fn signals(&mut self, signals: &Signals) {
        if self.room() > 0 && signals.woken.swap(false, Ordering::AcqRel) {
            self.push(0, bits::WOKEN);
        }
        let mut resolved = table::lock(&signals.resolved);
        let taken = resolved.len().min(self.room());
        for token in resolved.drain(..taken) {
            self.push(token, bits::RESOLVED);
        }
    }
}

fn timeout(nanoseconds: i64) -> Option<Duration> {
    u64::try_from(nanoseconds).ok().map(Duration::from_nanos)
}

/// Waits up to `timeout` nanoseconds (forever when negative, not at all when
/// 0) for events, and writes each into `destination` as its token, then its
/// bits. A wake is the event `(0, woken)`; a finished lookup is its token with
/// the resolved bit. Answers the number of events written.
#[must_use]
pub fn wait(handle: i64, timeout_nanoseconds: i64, destination: &mut [i64]) -> i64 {
    word((|| {
        let resource = poller(handle)?;
        let Resource::Poller(poller) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let mut collected = Collected {
            destination,
            count: 0,
        };
        if collected.room() == 0 {
            return Ok(0);
        }
        collected.signals(&poller.signals);
        // Something is already waiting to be seen, so the poll only adds what
        // is ready now.
        let limit = if collected.count > 0 {
            Some(Duration::ZERO)
        } else {
            timeout(timeout_nanoseconds)
        };
        let mut events = Events::with_capacity(collected.room());
        match table::lock(&poller.poll).poll(&mut events, limit) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
        for event in &events {
            if event.token() == WAKE {
                continue;
            }
            if collected.room() == 0 {
                break;
            }
            collected.push(length(event.token().0), event_bits(event));
        }
        collected.signals(&poller.signals);
        Ok(length(collected.count))
    })())
}

/// Ends a wait in progress on the poller from any thread, or the next one.
#[must_use]
pub fn wake(handle: i64) -> i64 {
    word((|| {
        let resource = poller(handle)?;
        let Resource::Poller(poller) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        poller.signals.woken.store(true, Ordering::Release);
        poller.signals.waker.wake()?;
        Ok(1)
    })())
}
