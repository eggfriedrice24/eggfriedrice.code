use efr_sandbox::FsAccess;
use landlock::{ABI, AccessFs};

use super::*;

#[test]
fn handled_rights_are_every_right_of_abi_9() {
    assert_eq!(access(&FsAccess::HANDLED), Some(AccessFs::from_all(ABI::V9)));
}

#[test]
fn device_rights_are_file_rights_only() {
    let device = access(&FsAccess::DEVICE).unwrap();
    assert_eq!(device, AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::IoctlDev);
    assert!(AccessFs::from_file(ABI::V9).contains(device));
}

#[test]
fn write_dir_rights_leave_out_devices_and_sockets() {
    let write = access(&FsAccess::WRITE_DIR).unwrap();
    for right in
        [AccessFs::MakeChar, AccessFs::MakeBlock, AccessFs::IoctlDev, AccessFs::ResolveUnix]
    {
        assert!(!write.contains(right), "{right:?}");
    }
}

#[test]
fn a_missing_rule_path_is_left_out() {
    assert!(open_rule_path(Path::new("/nonexistent/efr-sbx-rule")).unwrap().is_none());
    assert!(open_rule_path(Path::new("/")).unwrap().is_some());
}
