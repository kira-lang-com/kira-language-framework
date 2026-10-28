use klf_host_net::{self as net, Failure, bits};

const SECOND: i64 = 1_000_000_000;

fn text(bytes: &[u8]) -> &str {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap()
}

fn bound(handle: i64) -> String {
    let mut buffer = [0; 64];
    let count = net::local_address(handle, &mut buffer);
    assert!(count > 0, "local address failed: {count}");
    text(&buffer).to_owned()
}

/// Events seen but not yet asked for: readiness is reported once, when it
/// changes, so a wait for one token must keep what it saw of the others.
#[derive(Default)]
struct Seen(std::collections::HashMap<i64, i64>);

impl Seen {
    /// Waits until `token` has been reported with every bit of `wanted`.
    fn take(&mut self, poller: i64, token: i64, wanted: i64) -> i64 {
        let mut events = [0; 32];
        for _ in 0..20 {
            if let Some(bits) = self.0.get(&token).copied()
                && bits & wanted == wanted
            {
                self.0.remove(&token);
                return bits;
            }
            let count = net::poller_wait(poller, SECOND, &mut events);
            assert!(count >= 0, "wait failed: {count}");
            for event in events.chunks(2).take(usize::try_from(count).unwrap()) {
                *self.0.entry(event[0]).or_default() |= event[1];
            }
        }
        panic!("no event {wanted} for token {token}");
    }
}

#[test]
fn a_datagram_crosses_loopback_and_names_its_sender() {
    let mut seen = Seen::default();
    let poller = net::poller_create();
    let receiver = net::udp_bind(b"127.0.0.1:0");
    let sender = net::udp_bind(b"127.0.0.1:0");
    assert!(poller > 0 && receiver > 0 && sender > 0);
    assert_eq!(net::poller_watch(poller, receiver, bits::READABLE, 7), 1);
    let mut payload = [0; 16];
    let mut peer = [0; 64];
    assert_eq!(
        net::udp_receive(receiver, &mut payload, &mut peer),
        Failure::WouldBlock as i64
    );

    let at = bound(receiver);
    assert_eq!(net::udp_send(sender, at.as_bytes(), b"ping"), 4);
    seen.take(poller, 7, bits::READABLE);
    assert_eq!(net::udp_receive(receiver, &mut payload, &mut peer), 4);
    assert_eq!(&payload[..4], b"ping");
    assert_eq!(text(&peer), bound(sender));
    for handle in [receiver, sender, poller] {
        assert_eq!(net::close(handle), 1);
    }
}

#[test]
fn a_stream_connects_carries_bytes_and_ends() {
    let mut seen = Seen::default();
    let poller = net::poller_create();
    let listener = net::tcp_listen(b"127.0.0.1:0", 16);
    assert!(listener > 0);
    assert_eq!(net::poller_watch(poller, listener, bits::READABLE, 1), 1);
    let client = net::tcp_connect(bound(listener).as_bytes());
    assert!(client > 0);
    assert_eq!(
        net::poller_watch(poller, client, bits::READABLE | bits::WRITABLE, 2),
        1
    );

    seen.take(poller, 1, bits::READABLE);
    let mut peer = [0; 64];
    let server = net::tcp_accept(listener, &mut peer);
    assert!(server > 0, "accept failed: {server}");
    assert_eq!(text(&peer), bound(client));
    assert_eq!(net::poller_watch(poller, server, bits::READABLE, 3), 1);

    seen.take(poller, 2, bits::WRITABLE);
    assert_eq!(net::tcp_send(client, b"hello"), 5);
    assert_eq!(net::tcp_shutdown(client), 1);
    seen.take(poller, 3, bits::READABLE);
    let mut received = Vec::new();
    let mut buffer = [0; 8];
    loop {
        match net::tcp_receive(server, &mut buffer) {
            0 => break,
            count if count > 0 => {
                received.extend_from_slice(&buffer[..usize::try_from(count).unwrap()]);
            }
            count if count == Failure::WouldBlock as i64 => {
                seen.take(poller, 3, bits::READABLE);
            }
            failure => panic!("receive failed: {failure}"),
        }
    }
    assert_eq!(received, b"hello");
    for handle in [client, server, listener, poller] {
        assert_eq!(net::close(handle), 1);
    }
}

#[test]
fn a_refused_connection_reports_an_error() {
    let listener = net::tcp_listen(b"127.0.0.1:0", 1);
    let at = bound(listener);
    assert_eq!(net::close(listener), 1);
    let poller = net::poller_create();
    let client = net::tcp_connect(at.as_bytes());
    if client > 0 {
        assert_eq!(net::poller_watch(poller, client, bits::WRITABLE, 9), 1);
        let reported = Seen::default().take(poller, 9, 0);
        assert_ne!(reported & (bits::ERROR | bits::PEER_CLOSED), 0);
        let mut buffer = [0; 4];
        assert!(net::tcp_receive(client, &mut buffer) < 0);
    } else {
        assert_eq!(client, Failure::Refused as i64);
    }
}

#[test]
fn a_wake_from_another_thread_ends_a_wait() {
    let poller = net::poller_create();
    let waker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(net::poller_wake(poller), 1);
    });
    let mut events = [0; 4];
    assert_eq!(net::poller_wait(poller, -1, &mut events), 1);
    assert_eq!(&events[..2], &[0, bits::WOKEN]);
    waker.join().unwrap();
    assert_eq!(net::poller_wait(poller, 0, &mut events), 0);
}

#[test]
fn a_lookup_finishes_as_an_event_with_its_addresses() {
    let mut seen = Seen::default();
    let poller = net::poller_create();
    let lookup = net::resolve(poller, b"localhost", 5);
    assert!(lookup > 0);
    seen.take(poller, 5, bits::RESOLVED);
    let mut list = [0; 256];
    let count = net::resolved(lookup, &mut list);
    assert!(count > 0, "resolve failed: {count}");
    let found: Vec<&str> = list[..usize::try_from(count).unwrap()]
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| std::str::from_utf8(entry).unwrap())
        .collect();
    assert!(
        found
            .iter()
            .any(|address| *address == "127.0.0.1" || *address == "::1"),
        "{found:?}"
    );
}

#[test]
fn bad_handles_and_addresses_are_failure_words() {
    let mut buffer = [0; 4];
    assert_eq!(
        net::tcp_receive(9999, &mut buffer),
        Failure::InvalidHandle as i64
    );
    assert_eq!(
        net::udp_bind(b"not an address"),
        Failure::InvalidAddress as i64
    );
    let poller = net::poller_create();
    assert_eq!(
        net::tcp_receive(poller, &mut buffer),
        Failure::InvalidHandle as i64
    );
    assert_eq!(net::close(poller), 1);
    assert_eq!(net::close(poller), Failure::InvalidHandle as i64);
}

#[cfg(unix)]
#[test]
fn a_unix_stream_carries_bytes_both_ways() {
    use std::io::{Read, Write};

    let directory = std::env::temp_dir().join(format!("klf-net-unix-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("socket");
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let mut seen = Seen::default();
    let poller = net::poller_create();
    let client = net::unix_connect(path.as_os_str().as_encoded_bytes());
    assert!(client > 0, "connect failed: {client}");
    assert_eq!(net::poller_watch(poller, client, bits::READABLE, 4), 1);
    let (mut server, _) = listener.accept().unwrap();

    server.write_all(b"greeting").unwrap();
    seen.take(poller, 4, bits::READABLE);
    let mut buffer = [0; 16];
    assert_eq!(net::tcp_receive(client, &mut buffer), 8);
    assert_eq!(&buffer[..8], b"greeting");
    assert_eq!(net::tcp_send(client, b"answer"), 6);
    assert_eq!(net::tcp_shutdown(client), 1);
    let mut answer = Vec::new();
    server.read_to_end(&mut answer).unwrap();
    assert_eq!(answer, b"answer");
    assert_eq!(net::close(client), 1);
    assert_eq!(net::close(poller), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
