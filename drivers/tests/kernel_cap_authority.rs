//! Exercise the same authority object used at every World capability syscall.
#[path = "../../kernel/src/cap_authority.rs"]
mod cap_authority;
use cap_authority::CapAuthority;

#[test]
fn unbound_and_kernel_callers_never_gain_authority() {
    let mut authority = CapAuthority::new();
    for root in [None, Some(0), Some(0x1000), Some(0x2000)] {
        assert!(!authority.permits(root));
    }
    assert_eq!(authority.bind(0), Err(()));
    authority.bind(0x1000).unwrap();
    assert!(!authority.permits(None));
    assert!(!authority.permits(Some(0)));
}

#[test]
fn shared_root_threads_are_authorized_but_another_image_is_not() {
    let mut authority = CapAuthority::new();
    authority.bind(0x1000).unwrap();
    for _sibling in 0..4 {
        assert!(authority.permits(Some(0x1000)));
    }
    assert!(!authority.permits(Some(0x2000)));
}

#[test]
fn rebinding_cannot_transfer_or_erase_authority() {
    let mut authority = CapAuthority::new();
    authority.bind(0x1000).unwrap();
    for root in [0, 0x1000, 0x2000] {
        assert_eq!(authority.bind(root), Err(()));
        assert!(authority.permits(Some(0x1000)));
        assert!(!authority.permits(Some(0x2000)));
    }
}
