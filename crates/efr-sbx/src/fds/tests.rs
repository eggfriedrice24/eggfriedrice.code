use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, IntoRawFd};

use super::*;

#[test]
fn adopt_refuses_standard_streams() {
    for fd in 0..3 {
        assert_eq!(adopt(fd).map(drop).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn adopt_refuses_a_closed_number() {
    let error = adopt(4000).map(drop).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
}

#[test]
fn adopt_takes_an_open_number() {
    let raw = File::open("/dev/null").unwrap().into_raw_fd();
    let owned = adopt(raw).unwrap();
    assert_eq!(owned.as_raw_fd(), raw);
}

#[test]
fn dup_to_puts_a_copy_at_the_number() {
    let file = File::open("/dev/null").unwrap();
    let copy = dup_to(file.as_fd(), 900, true).unwrap();
    assert_eq!(copy.as_raw_fd(), 900);
    let flags = rustix::io::fcntl_getfd(&copy).unwrap();
    assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
}

#[test]
fn dup_to_refuses_its_own_number_and_the_standard_streams() {
    let file = File::open("/dev/null").unwrap();
    let raw = file.as_raw_fd();
    assert!(dup_to(file.as_fd(), raw, false).is_err());
    assert!(dup_to(file.as_fd(), 2, false).is_err());
}

#[test]
fn mark_inherited_cloexec_sets_the_flag() {
    let file = File::open("/dev/null").unwrap();
    rustix::io::fcntl_setfd(&file, rustix::io::FdFlags::empty()).unwrap();
    mark_inherited_cloexec(3).unwrap();
    let flags = rustix::io::fcntl_getfd(&file).unwrap();
    assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
}
