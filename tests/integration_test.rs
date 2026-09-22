use avt::Terminal;
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_integration_test_arbitrary_bytes_do_not_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..4096),
    ) {
        let mut terminal = Terminal::new(10, 4).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        terminal.feed(text.as_ref());
    }
}
