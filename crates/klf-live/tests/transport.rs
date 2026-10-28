//! The section 17.5 framing over a real socket: frames the wire codec produced
//! come back off a stream the peer wrote, and a push travels between two ends.

use klf_live::client::{Client, Pushed};
use klf_live::wire::{Frame, Hello, Push};
use klf_live::{Bundle, Device, VmEngine};
use std::net::TcpListener;
use std::thread;

fn module(answer: i64) -> String {
    format!(
        r#"module @Live lang "test" target "aarch64-apple-darwin" stage raw

func private @answer() -> i64 engine(runtime) {{
bb0():
  %value = const.int i64, {answer}
  return %value
}}

func private @main() -> i64 engine(runtime) {{
bb0():
  %value = call @answer()
  return %value
}}
"#
    )
}

/// A device listening on loopback, started with the app's first version and
/// answering every frame the way a device does.
fn device_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = listener.local_addr().expect("has an address").to_string();
    thread::spawn(move || {
        let bundle = Bundle {
            id: "bundle".to_owned(),
            debug: true,
        };
        let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle);
        let first = klf_kir::parse_module(&module(1)).expect("parses");
        if device.install(&first).is_err() {
            return;
        }
        device.serve(&listener, "test", &mut |_, _| {}, &mut |_| {});
    });
    address
}

#[test]
fn a_pushed_module_travels_from_a_client_to_a_device_and_back() {
    let address = device_server();
    let mut client = Client::connect(&address).expect("connects");
    let hello = client.handshake("bundle").expect("the device answers");
    assert_eq!(hello.version, klf_live::VERSION);

    match client
        .push("Live", &module(9))
        .expect("the exchange completes")
    {
        Pushed::Installed(replaced) => assert!(replaced > 0),
        Pushed::Refused(message) => panic!("the device refused it: {message}"),
    }
}

#[test]
fn a_device_serves_the_next_client_once_one_hangs_up() {
    let address = device_server();
    for answer in [9, 10] {
        let mut client = Client::connect(&address).expect("connects");
        client.handshake("bundle").expect("the device answers");
        match client
            .push("Live", &module(answer))
            .expect("the exchange completes")
        {
            Pushed::Installed(replaced) => assert_eq!(replaced, 2),
            Pushed::Refused(message) => panic!("the device refused it: {message}"),
        }
    }
}

#[test]
fn a_refusal_comes_back_as_a_result_rather_than_a_failure() {
    let address = device_server();
    let mut client = Client::connect(&address).expect("connects");
    client.handshake("bundle").expect("the device answers");
    match client
        .push("Live", "not a module")
        .expect("the device answered")
    {
        Pushed::Refused(message) => {
            assert!(message.contains("does not parse"), "it says why: {message}");
        }
        Pushed::Installed(_) => panic!("a broken module cannot install"),
    }
}

#[test]
fn a_hello_carries_its_version_and_the_module_layouts() {
    let hello = Hello {
        version: klf_live::VERSION,
        klf_version: "test".to_owned(),
        bundle: "bundle".to_owned(),
        layouts: vec![("Live".to_owned(), "abcd".to_owned())],
    };
    let frame = Frame::Hello(hello.clone());
    assert_eq!(frame.kind(), 1, "HELLO is type 1");

    let bytes = frame.encode();
    assert_eq!(Frame::framed_length(&bytes), Some(bytes.len()));
    assert_eq!(Frame::decode(&bytes).expect("round trips"), frame);
}

#[test]
fn every_frame_type_round_trips_through_the_codec() {
    let frames = [
        Frame::Hello(Hello::default()),
        Frame::Push(Push {
            module: "Live".to_owned(),
            kir: module(3),
        }),
        Frame::Ack(klf_live::Ack {
            module: "Live".to_owned(),
            replaced: 12,
        }),
        Frame::Nack(klf_live::Nack {
            module: "Live".to_owned(),
            reason: klf_live::Reason::OldCodeBusy,
            message: "both versions are running".to_owned(),
        }),
        Frame::Log("rebuilding".to_owned()),
    ];
    for frame in frames {
        let bytes = frame.encode();
        assert_eq!(Frame::framed_length(&bytes), Some(bytes.len()));
        assert_eq!(
            Frame::decode(&bytes).unwrap_or_else(|error| panic!("{frame:?}: {error}")),
            frame
        );
    }
}

#[test]
fn a_partial_frame_is_not_read_as_a_whole_one() {
    let bytes = Frame::Log("rebuilding".to_owned()).encode();
    assert_eq!(
        Frame::framed_length(&bytes[..2]),
        None,
        "too short to know the length"
    );
    assert_eq!(Frame::framed_length(&bytes[..bytes.len() - 1]), None);
    assert_eq!(Frame::framed_length(&bytes), Some(bytes.len()));
}
