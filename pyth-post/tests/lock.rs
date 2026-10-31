//! `PayerLock`: one holder per payer at a time, released on drop, independent per payer.

#![allow(clippy::unwrap_used, clippy::panic, clippy::disallowed_macros)]

use std::io::ErrorKind;

use pyth_post::PayerLock;
use solana_address::Address;

#[test]
fn test_one_holder_per_payer() {
    let dir = std::env::temp_dir().join(format!("pyth-post-lock-test-{}", std::process::id()));
    let (payer, other) = (Address::new_unique(), Address::new_unique());
    let held = PayerLock::acquire_in(&dir, &payer).unwrap();
    assert_eq!(held.path(), dir.join(format!("payer-{payer}.lock")));
    let err = PayerLock::acquire_in(&dir, &payer).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::WouldBlock);
    assert!(err.to_string().contains("one poster per payer"), "{err}");
    // Another payer is independent.
    let _other = PayerLock::acquire_in(&dir, &other).unwrap();
    drop(held);
    let _again = PayerLock::acquire_in(&dir, &payer).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}
