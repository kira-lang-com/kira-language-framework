//! Methods, bare member names, constants, generic enums and functions,
//! closures, and multi-payload enum cases.

mod support;

use klf_vm::Outcome;

#[test]
fn methods_take_receivers_and_static_methods_take_none() {
    let source = "
let step = 5

struct Counter {
    var value: Int

    function start(at: Int) -> Counter {
        return Counter { value = at }
    }

    function get(borrow self) -> Int {
        return value
    }

    function bump(borrow mut self, by: Int) {
        value += by
    }
}

extend Counter {
    function doubled(borrow self) -> Int {
        return get() * 2
    }
}

enum Shape {
    Square(Int)
    Rectangle(Int, Int)

    function area(borrow self) -> Int {
        match self {
            Square(side) -> { return side * side }
            Rectangle(width, height) -> { return width * height }
        }
    }
}

function run() -> Int {
    var counter = Counter.start(1)
    counter.bump(step)
    let shape: Shape = .Rectangle(3, 4)
    return counter.doubled() + shape.area()
}";
    assert_eq!(support::run(source, "run", &[]), Outcome::Return(Some(24)));
}

#[test]
fn generic_enums_are_instantiated_per_argument() {
    let source = "
struct Point {
    let x: Int
    let y: Int
}

function find(items: [Int], wanted: Int) -> Optional<Int> {
    var index = 0
    for item in items {
        if item == wanted { return .Some(index) }
        index += 1
    }
    return .None
}

function origin(present: Bool) -> Optional<Point> {
    if present { return .Some(Point { x = 7, y = 8 }) }
    return .None
}

function checked(value: Int) -> Result<Int, [Int]> {
    if value < 0 { return .Error([value, value]) }
    return .Ok(value)
}

function run() -> Int {
    var total = 0
    match find([4, 5, 6], 6) {
        Some(index) -> { total += index }
        None -> { total += 100 }
    }
    match origin(true) {
        Some(point) -> { total += point.x * point.y }
        None -> {}
    }
    match checked(-1) {
        Ok(value) -> { total += value }
        Error(values) -> { total += values.count * 1000 }
    }
    var cursor: Optional<Int> = .None
    if cursor.isNone() { total += 0 } else { total += 500000 }
    cursor = .Some(3)
    if !cursor.isSome() { total += 500000 }
    match cursor {
        Some(value) -> { total += value * 10000 }
        None -> {}
    }
    return total
}";
    assert_eq!(
        support::run(source, "run", &[]),
        Outcome::Return(Some(32058))
    );
}

#[test]
fn try_returns_the_failure_early_and_unwraps_success() {
    let source = "
struct Failure {
    let code: Int
}

function digit(value: Int) -> Result<Int, Failure> {
    if value > 9 { return .Error(Failure { code = value }) }
    return .Ok(value)
}

function sum(first: Int, second: Int) -> Result<Int, Failure> {
    let left = try digit(first)
    return .Ok(left + try digit(second))
}

function run() -> Int {
    var total = 0
    match sum(3, 4) {
        Ok(value) -> total += value
        Error -> total += 1000
    }
    match sum(3, 40) {
        Ok -> total += 1000
        Error(failure) -> total += failure.code * 100
    }
    return total
}";
    assert_eq!(
        support::run(source, "run", &[]),
        Outcome::Return(Some(4007))
    );
}

#[test]
fn a_generic_function_calls_its_bound_s_methods_on_each_instance() {
    let source = "
trait Doubled {
    function doubled(borrow self) -> Int
}
struct Pair {
    let left: Int
    let right: Int
}
extend Pair: Doubled {
    function doubled(borrow self) -> Int { return (left + right) * 2 }
}
extend Int: Doubled {
    function doubled(borrow self) -> Int { return self * 2 }
}
function twice<T: Doubled>(item: T) -> Int { return item.doubled() }
function run() -> Int {
    return twice(Pair { left = 10, right = 11 }) + twice(1)
}";
    assert_eq!(support::run(source, "run", &[]), Outcome::Return(Some(44)));
}

#[test]
fn closures_capture_locals_and_run_through_function_values() {
    let source = r#"
struct Total {
    let sum: Int
}
function apply(compute: (Int) -> Int, value: Int) -> Int { return compute(value) }
function total(compute: (Int) -> Total, value: Int) -> Total { return compute(value) }
function lookup<K, V>(key: K, compute: (K) -> V) -> V { return compute(key) }
function run() -> Int {
    let step = 2
    let words = ["a", "bb", "ccc"]
    let label = "seven"
    let scaled = apply({ value in value * step + words.count }, 20)
    let made = total({ value in
        let doubled = value * 2
        Total { sum = doubled + label.len }
    }, 3)
    let looked = lookup(4, { key in key + words[2].len })
    return scaled + made.sum + looked
}"#;
    assert_eq!(support::run(source, "run", &[]), Outcome::Return(Some(61)));
}

#[test]
fn a_closure_value_is_copied_and_released_with_its_captures() {
    let source = r#"
struct Holder {
    let compute: (Int) -> Int
}
function call(holder: Holder, value: Int) -> Int {
    let compute = holder.compute
    return compute(value)
}
function run() -> Int {
    var result = 0
    for round in 0..3 {
        let names = ["x", "yy"]
        let suffix = "abc"
        let compute: (Int) -> Int = { value in value + names[1].len + suffix.len }
        let holder = Holder { compute = compute }
        let again = compute
        result += again(round) + call(holder, 10)
    }
    return result
}"#;
    assert_eq!(support::run(source, "run", &[]), Outcome::Return(Some(63)));
}
