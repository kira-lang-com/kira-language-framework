#![cfg(unix)]

use klf_host_process::children::{self, STILL_RUNNING, Signal, Stream};

fn command(arguments: &[&str]) -> i64 {
    let handle = children::spawn_create();
    for argument in arguments {
        assert_eq!(children::spawn_argument(handle, argument.as_bytes()), 1);
    }
    handle
}

fn read_all(child: i64, stream: i64) -> Vec<u8> {
    let mut all = Vec::new();
    let mut chunk = [0_u8; 7];
    loop {
        let read = children::child_read(child, stream, &mut chunk, -1);
        assert!(read >= 0, "read failed with {read}");
        if read == 0 {
            return all;
        }
        all.extend_from_slice(&chunk[..usize::try_from(read).unwrap()]);
    }
}

#[test]
fn a_piped_child_is_read_and_waited_for() {
    let handle = command(&["/bin/sh", "-c", "printf \"$GREETING\"; printf oops >&2; exit 3"]);
    assert_eq!(children::spawn_clear_environment(handle), 1);
    assert_eq!(children::spawn_environment(handle, b"GREETING=hello there"), 1);
    assert_eq!(children::spawn_stream(handle, 1, Stream::Pipe), 1);
    assert_eq!(children::spawn_stream(handle, 2, Stream::Pipe), 1);
    let child = children::spawn_start(handle);
    assert!(child > 0);
    assert_eq!(read_all(child, 1), b"hello there");
    assert_eq!(read_all(child, 2), b"oops");
    assert_eq!(children::child_wait(child, -1), 3);
    assert_eq!(children::child_release(child), 1);
}

#[test]
fn input_goes_to_the_child_and_a_directory_is_its_own() {
    let handle = command(&["/bin/sh", "-c", "pwd; cat"]);
    assert_eq!(children::spawn_directory(handle, b"/tmp"), 1);
    assert_eq!(children::spawn_stream(handle, 0, Stream::Pipe), 1);
    assert_eq!(children::spawn_stream(handle, 1, Stream::Pipe), 1);
    let child = children::spawn_start(handle);
    assert_eq!(children::child_write(child, b"piped"), 5);
    assert_eq!(children::child_close_input(child), 1);
    let output = read_all(child, 1);
    assert!(output.ends_with(b"\npiped"), "{}", String::from_utf8_lossy(&output));
    assert_eq!(children::child_wait(child, -1), 0);
}

#[test]
fn a_running_child_times_out_and_a_signal_ends_it() {
    let child = children::spawn_start(command(&["/bin/sleep", "30"]));
    assert!(child > 0);
    assert_eq!(children::child_wait(child, 1_000_000), STILL_RUNNING);
    assert_eq!(children::child_signal(child, Signal::Kill), 1);
    assert_eq!(children::child_wait(child, -1), children::SIGNALLED - 9);
}

#[test]
fn a_missing_program_does_not_start() {
    let child = children::spawn_start(command(&["/no/such/program"]));
    assert_eq!(child, children::Failure::Missing as i64);
}

#[test]
fn wait_any_answers_each_ending_once() {
    let first = children::spawn_start(command(&["/bin/true"]));
    let second = children::spawn_start(command(&["/bin/false"]));
    let mut ended = Vec::new();
    while ended.len() < 2 {
        let child = children::child_wait_any(5_000_000_000);
        assert!(child > 0, "no child ended");
        if child == first || child == second {
            ended.push(child);
        }
    }
    ended.sort_unstable();
    let mut expected = vec![first, second];
    expected.sort_unstable();
    assert_eq!(ended, expected);
    assert_eq!(children::child_wait(first, 0), 0);
    assert_eq!(children::child_wait(second, 0), 1);
}
