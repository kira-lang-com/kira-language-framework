//! Name resolution through the platform's resolver, off the waiting thread.

use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};

use crate::poller::poller;
use crate::table::{self, Resource};
use crate::{Failure, length, word};

type Answer = Option<Result<Vec<std::net::IpAddr>, Failure>>;

pub(crate) struct Lookup {
    answer: Arc<Mutex<Answer>>,
}

/// Starts resolving `host`, a name with no port, on a worker thread, and
/// answers the lookup's handle. When it finishes, `poller` reports `token`
/// with the resolved bit.
#[must_use]
pub fn resolve(poller_handle: i64, host: &[u8], token: i64) -> i64 {
    word((|| {
        if token < 0 {
            return Err(Failure::InvalidHandle);
        }
        let host = std::str::from_utf8(host)
            .map_err(|_| Failure::InvalidAddress)?
            .to_owned();
        if host.is_empty() {
            return Err(Failure::InvalidAddress);
        }
        let resource = poller(poller_handle)?;
        let Resource::Poller(poller) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let signals = Arc::clone(&poller.signals);
        let answer = Arc::new(Mutex::new(None));
        let filled = Arc::clone(&answer);
        std::thread::Builder::new()
            .name("klf-net-resolve".into())
            .spawn(move || {
                // getaddrinfo needs a port; 0 asks for none in particular.
                let found = (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|addresses| addresses.map(|address| address.ip()).collect::<Vec<_>>())
                    .map_err(|_| Failure::NotFound)
                    .and_then(|addresses| {
                        if addresses.is_empty() {
                            Err(Failure::NotFound)
                        } else {
                            Ok(addresses)
                        }
                    });
                *table::lock(&filled) = Some(found);
                signals.resolved(token);
            })
            .map_err(|_| Failure::Io)?;
        Ok(table::insert(Resource::Lookup(Lookup { answer })))
    })())
}

/// The addresses a finished lookup found, each a NUL-ended text with no port,
/// copied only when all fit; answers their length, or would-block while the
/// lookup runs.
#[must_use]
pub fn resolved(handle: i64, destination: &mut [u8]) -> i64 {
    word((|| {
        let resource = table::get(handle)?;
        let Resource::Lookup(lookup) = &*resource else {
            return Err(Failure::InvalidHandle);
        };
        let answer = table::lock(&lookup.answer);
        let addresses = answer
            .as_ref()
            .ok_or(Failure::WouldBlock)?
            .as_ref()
            .map_err(|failure| *failure)?;
        let mut text = Vec::new();
        for address in addresses {
            text.extend_from_slice(address.to_string().as_bytes());
            text.push(0);
        }
        if text.len() <= destination.len() {
            destination[..text.len()].copy_from_slice(&text);
        }
        Ok(length(text.len()))
    })())
}
