use avt::Terminal;
use rand::RngExt;

#[test]
fn integration_test_random_input_does_not_panic() {
    let mut bytes = [0u8; 1024 * 512];
    rand::rng().fill(&mut bytes[..]);
    let mut terminal = Terminal::new(10, 4).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    terminal.feed(text.as_ref());
}
