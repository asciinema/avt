use avt::Vt;
use rand::RngCore;

#[test]
fn feed_str() {
    let mut bytes = [0u8; 1024 * 512];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut vt = Vt::new(10, 4);
    let str = String::from_utf8_lossy(&bytes);
    vt.feed_str(&str);
    // no assertions - just check it doesn't panic on random input
}
