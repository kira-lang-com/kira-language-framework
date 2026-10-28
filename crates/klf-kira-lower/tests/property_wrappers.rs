//! Property wrappers (`KLF-CLI.md` section 2).

mod support;

use klf_vm::Outcome;

#[test]
fn wrapped_properties_read_write_and_initialize_through_the_wrapper() {
    let source = "
@PropertyWrapper
struct Scaled<Value> {
    var wrappedValue: Value
    let factor: Int = 1
    let label: String = \"\"

    function scaled(borrow self) -> Int {
        return wrappedValue * factor
    }
}

struct Settings {
    @Scaled(factor: 10)
    var depth: Int = 3

    @Scaled
    var width: Int = 4

    let plain: Int

    function total(borrow self) -> Int {
        return depth + width + plain
    }

    function grow(borrow mut self) {
        depth += 1
    }
}

function run() -> Int {
    var settings = Settings { plain = 100, width = 7 }
    settings.grow()
    let scaled = settings._depth.scaled()
    return settings.total() * 1000 + scaled
}";
    assert_eq!(
        support::run(source, "run", &[]),
        Outcome::Return(Some(111_040))
    );
}
