use klf_host_files::handles::{self, Access, Failure};

fn scratch(name: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("klf-handles-{}-{name}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned().into_bytes()
}

#[test]
fn positional_writes_and_reads_round_trip() {
    let path = scratch("round-trip");
    let handle = handles::open(&path, Access::CreateNew);
    assert!(handle > 0, "opened: {handle}");
    assert_eq!(handles::write_at(handle, 0, b"hello world"), 11);
    assert_eq!(handles::write_at(handle, 6, b"kira!"), 5);
    assert_eq!(handles::write_at(handle, 20, b"end"), 3);
    assert_eq!(handles::sync(handle), 1);
    assert_eq!(handles::size(handle), 23);

    let mut read = [0; 11];
    assert_eq!(handles::read_at(handle, 0, &mut read), 11);
    assert_eq!(&read, b"hello kira!");
    let mut tail = [0; 8];
    assert_eq!(handles::read_at(handle, 20, &mut tail), 3);

    assert_eq!(handles::truncate(handle, 5), 1);
    assert_eq!(handles::size(handle), 5);
    assert_eq!(handles::close(handle), 1);
    assert_eq!(handles::close(handle), Failure::InvalidHandle as i64);
    assert_eq!(handles::open(&path, Access::CreateNew), Failure::Exists as i64);
    let _ = std::fs::remove_file(String::from_utf8(path).unwrap_or_default());
}

#[test]
fn a_missing_file_does_not_open_for_reading() {
    assert_eq!(
        handles::open(&scratch("missing"), Access::Read),
        Failure::Missing as i64
    );
}

#[test]
fn a_lock_is_held_until_its_handle_closes() {
    let path = scratch("lock");
    let first = handles::open(&path, Access::ReadWrite);
    let second = handles::open(&path, Access::ReadWrite);
    assert!(first > 0 && second > 0, "opened: {first} {second}");
    assert_eq!(handles::lock(first, 0), 1);
    assert_eq!(handles::lock(second, 0), 0);
    assert_eq!(handles::close(first), 1);
    assert_eq!(handles::lock(second, 1), 1);
    assert_eq!(handles::close(second), 1);
    assert_eq!(handles::lock(second, 0), Failure::InvalidHandle as i64);
    let _ = std::fs::remove_file(String::from_utf8(path).unwrap_or_default());
}

#[cfg(unix)]
#[test]
fn a_descriptor_a_library_handed_over_closes_once() {
    use std::os::fd::IntoRawFd;
    let path = scratch("descriptor");
    let file = std::fs::File::create(String::from_utf8(path.clone()).expect("utf-8 path")).expect("created");
    let descriptor = i64::from(file.into_raw_fd());
    assert_eq!(handles::close_descriptor(descriptor), 1);
    assert_eq!(handles::close_descriptor(descriptor), Failure::InvalidHandle as i64);
    assert_eq!(handles::close_descriptor(-1), Failure::InvalidHandle as i64);
    let _ = std::fs::remove_file(String::from_utf8(path).expect("utf-8 path"));
}
