use super::*;

#[test]
fn line_breaks_of_every_kind_end_lines() {
    let index = LineIndex::new("a\nb\r\nc\rd");
    assert_eq!(index.line_count(), 4);
    assert_eq!(index.line_start(1), 2);
    assert_eq!(index.line_start(2), 5);
    assert_eq!(index.line_start(3), 7);
    assert_eq!(index.line_end(1), 3);
    assert_eq!(index.position(4, Encoding::Utf16), Position::new(1, 1));
}

#[test]
fn columns_count_the_negotiated_code_units() {
    // `é` is two UTF-8 bytes and one UTF-16 unit; `𝄞` is four bytes and a
    // surrogate pair.
    let text = "aé𝄞b";
    let index = LineIndex::new(text);
    let b = u32::try_from(text.find('b').unwrap()).unwrap();
    assert_eq!(index.position(b, Encoding::Utf8), Position::new(0, 7));
    assert_eq!(index.position(b, Encoding::Utf16), Position::new(0, 4));
    assert_eq!(index.position(b, Encoding::Utf32), Position::new(0, 3));
    for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
        assert_eq!(index.offset(index.position(b, encoding), encoding), b);
    }
    assert_eq!(index.line_width(0, Encoding::Utf16), 5);
}

#[test]
fn positions_inside_a_character_clamp_to_its_start() {
    let index = LineIndex::new("𝄞x");
    assert_eq!(index.offset(Position::new(0, 1), Encoding::Utf16), 0);
    assert_eq!(index.offset(Position::new(0, 2), Encoding::Utf16), 4);
    assert_eq!(index.position(2, Encoding::Utf16), Position::new(0, 0));
}

#[test]
fn positions_past_the_end_clamp() {
    let index = LineIndex::new("ab\ncd");
    assert_eq!(index.offset(Position::new(0, 9), Encoding::Utf16), 2);
    assert_eq!(index.offset(Position::new(7, 0), Encoding::Utf16), 5);
    assert_eq!(index.position(99, Encoding::Utf16), Position::new(1, 2));
}

#[test]
fn incremental_edits_apply_in_order() {
    let mut document = Document::new("let x = 1\nlet y = 2\n");
    document
        .edit(
            Some(Range::new(Position::new(1, 4), Position::new(1, 5))),
            "zé",
            Encoding::Utf16,
        )
        .unwrap();
    assert_eq!(document.text(), "let x = 1\nlet zé = 2\n");
    document
        .edit(
            Some(Range::new(Position::new(1, 6), Position::new(1, 6))),
            "!",
            Encoding::Utf16,
        )
        .unwrap();
    assert_eq!(document.text(), "let x = 1\nlet zé! = 2\n");
    document.edit(None, "new", Encoding::Utf16).unwrap();
    assert_eq!(document.text(), "new");
    assert_eq!(
        document.edit(
            Some(Range::new(Position::new(0, 2), Position::new(0, 1))),
            "",
            Encoding::Utf16
        ),
        Err(EditError::Reversed)
    );
}

#[test]
fn negotiation_prefers_utf8() {
    assert_eq!(
        Encoding::negotiate(&[Encoding::Utf16, Encoding::Utf8]),
        Encoding::Utf8
    );
    assert_eq!(Encoding::negotiate(&[Encoding::Utf32]), Encoding::Utf32);
    assert_eq!(Encoding::negotiate(&[]), Encoding::Utf16);
}
