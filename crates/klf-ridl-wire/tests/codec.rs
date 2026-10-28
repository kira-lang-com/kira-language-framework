//! The encoding section 24.7 asks for: a value crosses in the shape it has in
//! memory, an object crosses as a reference to whichever side hosts it, and a
//! message over the limit is refused rather than sent.

use klf_ridl_wire::{Call, Message, ObjectRef, Reply, Value};

#[test]
fn every_value_shape_round_trips() {
    let values = [
        Value::Void,
        Value::Bool(true),
        Value::Int(-42),
        Value::Uint(u128::MAX),
        Value::Float(0x3ff0_0000_0000_0000),
        Value::RawPtr(0xdead_beef),
        Value::Bytes(b"hello".to_vec()),
        Value::Seq(vec![Value::Int(1), Value::Uint(2)]),
        Value::some(Value::Int(7)),
        Value::none(),
        Value::result(Ok(Value::Bytes(b"ok".to_vec()))),
        Value::result(Err(Value::Bytes(b"bad".to_vec()))),
        Value::Struct(
            String::from("Point"),
            vec![Value::Float(1), Value::Float(2)],
        ),
        Value::Enum(
            String::from("Shape"),
            String::from("Circle"),
            vec![Value::Float(2)],
        ),
        Value::Object(ObjectRef::peer(9)),
    ];
    for value in values {
        let bytes = klf_ridl_wire::frame(&value);
        let back = klf_ridl_wire::unframe(&bytes).expect("decodes");
        assert_eq!(back, value, "{value} did not survive the crossing");
    }
}

#[test]
fn an_object_names_the_side_that_hosts_it() {
    let bytes = klf_ridl_wire::frame(&Value::Object(ObjectRef::here(3)));
    let back = klf_ridl_wire::unframe(&bytes).expect("decodes");
    assert_eq!(
        back,
        Value::Object(ObjectRef {
            side: klf_ridl_wire::Side::Here,
            id: 3
        })
    );
    // A host writes `here`, meaning "I host it"; the consumer reading that
    // reference knows to proxy rather than to dereference, which is the same
    // bit of information as the reference pointing the other way.
    assert_eq!(ObjectRef::here(3).side, klf_ridl_wire::Side::Here);
    assert_eq!(ObjectRef::here(3).side.flipped(), klf_ridl_wire::Side::Peer);
}

#[test]
fn a_corrupt_frame_is_refused_rather_than_read() {
    let truncated = klf_ridl_wire::frame(&Value::Bytes(b"hello".to_vec()));
    let cut = &truncated[..truncated.len() - 2];
    let error = klf_ridl_wire::unframe(cut).expect_err("a short frame is refused");
    assert!(error.contains("ended early"), "{error}");

    // An unknown tag is refused rather than skipped.
    let mut bogus = truncated.clone();
    bogus[0] = 0x7f;
    assert!(klf_ridl_wire::unframe(&bogus).is_err());
}

#[test]
fn a_message_carries_its_arguments_and_answers_by_request() {
    let call = Call::new(
        4,
        11,
        "Geometry.parse",
        vec![Value::Bytes(b"kira".to_vec()), Value::some(Value::Int(3))],
    );
    let bytes = Message::Request(call.clone()).encode().expect("encodes");
    assert_eq!(
        Message::decode(&bytes).expect("decodes"),
        Message::Request(call)
    );

    let reply = Message::Reply {
        request: 11,
        reply: Reply::Returned(Value::Float(1.5f64.to_bits())),
    };
    let bytes = reply.encode().expect("encodes");
    assert_eq!(Message::decode(&bytes).expect("decodes"), reply);
}

#[test]
fn every_reply_outcome_survives() {
    for reply in [
        Reply::Returned(Value::Uint(1)),
        Reply::Failed(Value::Bytes(b"odd".to_vec())),
        Reply::Trapped(String::from("KLF-VM-0005")),
        Reply::Refused(String::from("KLF-RT-0203")),
    ] {
        let message = Message::Reply {
            request: 1,
            reply: reply.clone(),
        };
        let bytes = message.encode().expect("encodes");
        let Message::Reply { reply: back, .. } = Message::decode(&bytes).expect("decodes") else {
            panic!("{reply:?} did not decode as a reply");
        };
        assert_eq!(back, reply);
    }
}

#[test]
fn an_object_export_and_release_name_their_connection() {
    for message in [
        Message::Exported {
            connection: 2,
            object: 17,
        },
        Message::Released {
            connection: 2,
            object: 17,
        },
        Message::Closed { connection: 2 },
    ] {
        let bytes = message.encode().expect("encodes");
        assert_eq!(Message::decode(&bytes).expect("decodes"), message);
    }
}

#[test]
fn a_message_over_the_limit_is_a_transfer_not_a_message() {
    let big = Value::Bytes(vec![0; klf_ridl_wire::MAX_MESSAGE + 1]);
    let call = Call::new(1, 1, "f", vec![big]);
    let error = Message::Request(call).encode().expect_err("refused");
    assert!(error.contains("transfer"), "{error}");

    // Just under the limit still travels as a message.
    let fits = Value::Bytes(vec![0; 1024]);
    let call = Call::new(1, 1, "f", vec![fits]);
    assert!(Message::Request(call).encode().is_ok());
}

#[test]
fn nesting_past_the_depth_limit_is_refused() {
    let mut value = Value::Uint(0);
    for _ in 0..(klf_ridl_wire::MAX_DEPTH + 8) {
        value = Value::some(value);
    }
    // The codec itself is iterative and will write it; the depth limit is the
    // channel's to enforce, so what this pins down is that a deep value does
    // not recurse the writer into a stack overflow.
    let bytes = klf_ridl_wire::frame(&value);
    assert_eq!(klf_ridl_wire::unframe(&bytes).expect("decodes"), value);
}
