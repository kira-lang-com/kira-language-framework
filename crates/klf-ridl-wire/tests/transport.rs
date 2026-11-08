#![cfg(unix)]
#![allow(unsafe_code)]

use klf_ridl_wire::transport::Connection;
use klf_ridl_wire::{Message, ObjectRef, Reply, Value};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

#[test]
fn isolated_peers_call_each_other_without_global_socket_names() {
    let (host, plugin) = Connection::pair(42).unwrap();
    let serving = std::thread::spawn(move || {
        plugin
            .serve_once(|call| {
                assert_eq!(call.connection, 42);
                assert_eq!(call.function, "Harmony.currentWorkspace");
                assert_eq!(call.arguments, vec![Value::Uint(3)]);
                Reply::Returned(Value::Bytes(b"workspace".to_vec()))
            })
            .unwrap();
    });
    let answer = host
        .call("Harmony.currentWorkspace", vec![Value::Uint(3)])
        .unwrap();
    assert_eq!(answer, Reply::Returned(Value::Bytes(b"workspace".to_vec())));
    serving.join().unwrap();
}

#[test]
fn nested_request_can_be_served_without_waiting_on_original_channel() {
    let (host, plugin) = Connection::pair(8).unwrap();
    let host = Arc::new(host);
    let plugin = Arc::new(plugin);
    let host_receiver = Arc::clone(&host);
    let host_server = std::thread::spawn(move || {
        host_receiver
            .serve_once(|call| {
                assert_eq!(call.function, "Host.settings");
                Reply::Returned(Value::Uint(21))
            })
            .unwrap();
    });
    let plugin_server = std::thread::spawn(move || {
        plugin
            .serve_once(|call| {
                assert_eq!(call.function, "Plugin.start");
                plugin.call("Host.settings", vec![]).unwrap()
            })
            .unwrap();
    });
    assert_eq!(
        host.call("Plugin.start", vec![]).unwrap(),
        Reply::Returned(Value::Uint(21))
    );
    plugin_server.join().unwrap();
    host_server.join().unwrap();
}

#[test]
fn a_shared_resource_arrives_as_a_new_descriptor() {
    let (host, plugin) = Connection::pair(91).unwrap();
    let original = File::open("Cargo.toml").unwrap();
    let descriptor: OwnedFd = original.into();
    host.export_resource(17, &descriptor).unwrap();
    let received = plugin.receive().unwrap();
    assert!(matches!(
        received.message,
        Message::Exported {
            connection: 91,
            object: 17
        }
    ));
    let shared = received.resource.unwrap();
    assert_ne!(shared.as_raw_fd(), descriptor.as_raw_fd());
    let mut owned: File = shared.into();
    let mut data = Vec::new();
    owned.read_to_end(&mut data).unwrap();
    assert!(data.starts_with(b"[package]") || data.starts_with(b"[workspace]"));
    drop(descriptor);
}

#[test]
fn export_is_tied_to_the_connection_that_receives_it() {
    let (host_a, plugin_a) = Connection::pair(101).unwrap();
    let (host_b, plugin_b) = Connection::pair(102).unwrap();
    host_a.release(9).unwrap();
    host_b.release(10).unwrap();
    assert!(matches!(
        plugin_a.receive().unwrap().message,
        Message::Released { object: 9, .. }
    ));
    assert!(matches!(
        plugin_b.receive().unwrap().message,
        Message::Released { object: 10, .. }
    ));
}

#[test]
fn raw_pointers_cannot_be_sent_to_another_process() {
    let (host, _plugin) = Connection::pair(8).unwrap();
    let error = host
        .call("Host.deref", vec![Value::RawPtr(0x1234)])
        .unwrap_err();
    assert!(error.to_string().contains("raw pointers"));
}

#[test]
fn excessive_nesting_is_refused_before_the_peer_sees_it() {
    let (host, _plugin) = Connection::pair(8).unwrap();
    let mut value = Value::Void;
    for _ in 0..65 {
        value = Value::some(value);
    }
    let error = host.call("Host.deep", vec![value]).unwrap_err();
    assert!(error.to_string().contains("KLF-RT-0204"));
}

#[test]
fn remote_object_references_change_perspective_on_receive() {
    let (host, plugin) = Connection::pair(54).unwrap();
    let worker = std::thread::spawn(move || {
        plugin
            .serve_once(|call| {
                assert_eq!(
                    call.arguments,
                    vec![Value::some(Value::Object(ObjectRef::peer(7)))]
                );
                Reply::Returned(Value::Object(ObjectRef::peer(7)))
            })
            .unwrap();
    });
    let reply = host
        .call(
            "Plugin.visit",
            vec![Value::some(Value::Object(ObjectRef::here(7)))],
        )
        .unwrap();
    assert_eq!(reply, Reply::Returned(Value::Object(ObjectRef::here(7))));
    worker.join().unwrap();
}

#[test]
fn dropping_the_last_proxy_releases_the_exported_object() {
    let (host, plugin) = Connection::pair(55).unwrap();
    let plugin = Arc::new(plugin);
    host.export_object(81).unwrap();
    assert!(matches!(
        plugin.receive().unwrap().message,
        Message::Exported { object: 81, .. }
    ));
    let proxy = plugin.proxy(81);
    let copy = proxy.clone();
    drop(proxy);
    drop(copy);
    assert!(matches!(
        host.receive().unwrap().message,
        Message::Released { object: 81, .. }
    ));
}

#[test]
fn caller_gets_eof_when_peer_dies_before_replying() {
    let (host, plugin) = Connection::pair(77).unwrap();
    let peer = std::thread::spawn(move || {
        let unanswered = plugin.receive().unwrap();
        assert!(matches!(unanswered.message, Message::Request(_)));
        drop(unanswered);
    });
    let error = host.call("Plugin.noReply", vec![]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    peer.join().unwrap();
}

#[test]
fn a_peer_cannot_attach_multiple_descriptors_to_one_resource_message() {
    let (mut sender, receiver) = UnixStream::pair().unwrap();
    let connection = Connection::from_stream(receiver, 70).unwrap();
    let message = Message::Exported {
        connection: 70,
        object: 9,
    }
    .encode()
    .unwrap();
    let length = (message.len() as u32).to_le_bytes();
    let mut iov = libc::iovec {
        iov_base: length.as_ptr().cast_mut().cast(),
        iov_len: length.len(),
    };
    let first = File::open("Cargo.toml").unwrap();
    let second = File::open("Cargo.toml").unwrap();
    let descriptors = [first.as_raw_fd(), second.as_raw_fd()];
    let mut control_buffer = [0usize; 8];
    let mut control: libc::msghdr = unsafe { std::mem::zeroed() };
    control.msg_iov = &raw mut iov;
    control.msg_iovlen = 1;
    control.msg_control = control_buffer.as_mut_ptr().cast();
    control.msg_controllen = unsafe { libc::CMSG_SPACE(8) as _ };
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const control);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(8) as _;
        std::ptr::copy_nonoverlapping(
            descriptors.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            8,
        );
        assert_eq!(libc::sendmsg(sender.as_raw_fd(), &raw const control, 0), 4);
    }
    sender.write_all(&message).unwrap();
    let error = connection.receive().err().unwrap();
    assert!(error.to_string().contains("multiple descriptors"));
}
