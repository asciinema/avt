#[cfg(test)]
extern crate quickcheck;
#[cfg(test)]
#[macro_use(quickcheck)]
extern crate quickcheck_macros;

use rand::RngCore;

#[test]
fn feed_str() {
    let mut bytes = [0u8; 1024 * 512];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut vt = vt::VT::new(10, 4);
    let str = String::from_utf8_lossy(&bytes);
    vt.feed_str(&str);
    // no assertions - just check it doesn't panic on random input
}

#[quickcheck]
fn feed(bytes: Vec<u8>) -> bool {
    let mut vt = vt::VT::new(10, 4);

    for b in bytes.iter() {
        vt.feed((*b) as char);
    }

    vt.get_cursor_x() <= 10
}