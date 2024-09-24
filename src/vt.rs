use crate::dump::Dump;
use crate::line::Line;
use crate::parser::Parser;
use crate::terminal::{Cursor, Scrollback, Terminal};

#[derive(Debug)]
pub struct Vt {
    parser: Parser,
    terminal: Terminal,
}

impl Vt {
    pub fn builder() -> Builder {
        Builder::default()
    }

    pub fn new(cols: usize, rows: usize) -> Vt {
        Self::builder().size(cols, rows).build()
    }

    pub fn feed_str(
        &mut self,
        s: &str,
    ) -> (
        Vec<usize>,
        bool,
        Scrollback<impl Iterator<Item = Line> + '_>,
    ) {
        self.parser.feed_str(s, &mut self.terminal);
        let (dirty_lines, resized) = self.terminal.changes();
        let scrollback = self.terminal.gc();

        // TODO use named struct instead
        (dirty_lines, resized, scrollback)
    }

    pub fn feed(&mut self, input: char) {
        self.parser.feed(input, &mut self.terminal);
    }

    pub fn size(&self) -> (usize, usize) {
        (self.terminal.cols, self.terminal.rows)
    }

    pub fn view(&self) -> &[Line] {
        self.terminal.view()
    }

    pub fn lines(&self) -> &[Line] {
        self.terminal.lines()
    }

    pub fn line(&self, n: usize) -> &Line {
        self.terminal.line(n)
    }

    pub fn text(&self) -> Vec<String> {
        self.terminal.text()
    }

    pub fn cursor(&self) -> Cursor {
        self.terminal.cursor()
    }

    pub fn cursor_key_app_mode(&self) -> bool {
        self.terminal.cursor_key_app_mode()
    }

    pub fn dump(&self) -> String {
        let mut seq = self.terminal.dump();
        seq.push_str(&self.parser.dump());

        seq
    }
}

pub struct Builder {
    size: (usize, usize),
    scrollback_limit: Option<usize>,
    resizable: bool,
}

impl Builder {
    pub fn size(&mut self, cols: usize, rows: usize) -> &mut Self {
        self.size = (cols, rows);

        self
    }

    pub fn scrollback_limit(&mut self, limit: usize) -> &mut Self {
        self.scrollback_limit = Some(limit);

        self
    }

    pub fn resizable(&mut self, resizable: bool) -> &mut Self {
        self.resizable = resizable;

        self
    }

    pub fn build(&self) -> Vt {
        Vt {
            parser: Parser::new(),
            terminal: Terminal::new(self.size, self.scrollback_limit, self.resizable),
        }
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            size: (80, 24),
            scrollback_limit: None,
            resizable: false,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::Vt;
    use crate::line::Line;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use std::env;
    use std::fs;

    #[test]
    fn auto_wrap_mode() {
        // auto wrap

        let mut vt = Vt::new(4, 4);

        vt.feed_str("\x1b[?7h");
        vt.feed_str("abcdef");

        assert_eq!(text(&vt), "abcd\nef|\n\n");

        // no auto wrap

        let mut vt = Vt::new(4, 4);

        vt.feed_str("\x1b[?7l");
        vt.feed_str("abcdef");

        assert_eq!(text(&vt), "abc|f\n\n\n");
    }

    #[test]
    fn print_at_the_end_of_the_screen() {
        // default margins, print at the bottom

        let mut vt = Vt::new(4, 6);

        let input = "xxxxxxxxxx\x1b[50;1Hyyy\x1b[50Czzz";
        vt.feed_str(input);

        assert_eq!(text(&vt), "xxxx\nxx\n\n\nyyyz\nzz|");

        // custom top margin, print above it

        let mut vt = Vt::new(4, 6);

        let input = "\nxxxxxxxxxx\x1b[2;4r\x1b[1;1Hyyy\x1b[50Czzz";

        vt.feed_str(input);

        assert_eq!(text(&vt), "yyyz\nzz|xx\nxxxx\nxx\n\n");

        // custom bottom margin, print below it

        let mut vt = Vt::new(4, 6);

        let input = "\x1b[;3rxxxxxxxxxx\x1b[50;1Hyyy\x1b[50Czzz";

        vt.feed_str(input);

        assert_eq!(text(&vt), "xxxx\nxxxx\nxx\n\n\nzz|yz");
    }

    #[test]
    fn execute_lf() {
        let mut vt = build_vt(8, 2, 3, 0, "abc");

        vt.feed_str("\n");

        assert_eq!(vt.cursor(), (3, 1));
        assert_eq!(text(&vt), "abc\n   |");

        vt.feed_str("d\n");

        assert_eq!(vt.cursor(), (4, 1));
        assert_eq!(text(&vt), "   d\n    |");
    }

    #[test]
    fn execute_ri() {
        let mut vt = build_vt(8, 5, 0, 0, "abcd\r\nefgh\r\nijkl\r\nmnop\r\nqrst");

        vt.feed_str("\x1bM"); // RI

        assert_eq!(text(&vt), "|\nabcd\nefgh\nijkl\nmnop");

        vt.feed_str("\x1b[3;4r"); // use smaller scroll region
        vt.feed_str("\x1b[3;1H"); // place cursor on top margin
        vt.feed_str("\x1bM"); // RI

        assert_eq!(text(&vt), "\nabcd\n|\nefgh\nmnop");
    }

    #[test]
    fn execute_su() {
        // short lines, default margins

        let mut vt = Vt::new(4, 6);
        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2S");
        assert_eq!(text(&vt), "cc\ndd\nee\nff\n\n  |");

        // short lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2S");

        assert_eq!(text(&vt), "|aa\ndd\nee\n\n\nff");

        // wrapped lines, default margins

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2S");

        assert_eq!(text(&vt), "bbbb\nbb\ncccc\ncc\n\n  |");
        assert_eq!(wrapped(&vt), vec![true, false, true, false, false, false]);

        // wrapped lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2S");

        assert_eq!(text(&vt), "|aaaa\nbb\ncccc\n\n\ncc");
        assert_eq!(wrapped(&vt), vec![false, false, false, false, false, false]);
    }

    #[test]
    fn execute_sd() {
        // short lines, default margins

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2T");

        assert_eq!(text(&vt), "\n\naa\nbb\ncc\ndd|");

        // short lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aa\r\nbb\r\ncc\r\ndd\r\nee\r\nff");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2T");

        assert_eq!(text(&vt), "|aa\n\n\nbb\ncc\nff");

        // wrapped lines, default margins

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2T");

        assert_eq!(text(&vt), "\n\naaaa\naa\nbbbb\nbb|");
        assert_eq!(wrapped(&vt), vec![false, false, true, false, true, false]);

        // wrapped lines, margins at 1 (top) and 4 (bottom)

        let mut vt = Vt::new(4, 6);

        vt.feed_str("aaaaaa\r\nbbbbbb\r\ncccccc");
        vt.feed_str("\x1b[2;5r");
        vt.feed_str("\x1b[1;1H");
        vt.feed_str("\x1b[2T");

        assert_eq!(text(&vt), "|aaaa\n\n\naa\nbbbb\ncc");
        assert_eq!(wrapped(&vt), vec![false, false, false, false, false, false]);
    }

    #[test]
    fn execute_bs() {
        let mut vt = Vt::new(4, 2);

        vt.feed_str("a");
        vt.feed_str("\x08");

        assert_eq!(text(&vt), "|a\n");

        vt.feed_str("\x08");

        assert_eq!(text(&vt), "|a\n");

        vt.feed_str("abcd");
        vt.feed_str("\x08");

        assert_eq!(text(&vt), "ab|cd\n");

        vt.feed_str("cdef");
        vt.feed_str("\x08");

        assert_eq!(text(&vt), "abcd\ne|f");

        vt.feed_str("\x08");

        assert_eq!(text(&vt), "abcd\n|ef");

        vt.feed_str("\x08");

        assert_eq!(text(&vt), "abcd\n|ef");
    }

    #[test]
    fn execute_cup() {
        let mut vt = Vt::new(4, 2);

        vt.feed_str("abc\r\ndef");
        vt.feed_str("\x1b[1;1;H");

        assert_eq!(vt.cursor(), (0, 0));

        vt.feed_str("\x1b[10;10;H");

        assert_eq!(vt.cursor(), (3, 1));
    }

    #[test]
    fn execute_cuu() {
        let mut vt = Vt::new(8, 4);

        vt.feed_str("abcd\n\n\n");
        vt.feed_str("\x1b[A");

        assert_eq!(vt.cursor(), (4, 2));

        vt.feed_str("\x1b[2A");

        assert_eq!(vt.cursor(), (4, 0));
    }

    #[test]
    fn execute_cpl() {
        let mut vt = Vt::new(8, 4);

        vt.feed_str("abcd\r\n\r\n\r\nef");

        assert_eq!(vt.cursor(), (2, 3));

        vt.feed_str("\x1b[F");

        assert_eq!(vt.cursor(), (0, 2));

        vt.feed_str("\x1b[2F");

        assert_eq!(vt.cursor(), (0, 0));
    }

    #[test]
    fn execute_cnl() {
        let mut vt = Vt::new(4, 4);

        vt.feed_str("ab");
        vt.feed_str("\x1b[E");

        assert_eq!(vt.cursor(), (0, 1));

        vt.feed_str("\x1b[3E");

        assert_eq!(vt.cursor(), (0, 3));
    }

    #[test]
    fn execute_vpa() {
        let mut vt = Vt::new(4, 4);

        vt.feed_str("\r\n\r\naaa\r\nbbb");
        vt.feed_str("\x1b[d");

        assert_eq!(vt.cursor(), (3, 0));

        vt.feed_str("\x1b[10d");

        assert_eq!(vt.cursor(), (3, 3));
    }

    #[test]
    fn execute_cud() {
        let mut vt = Vt::new(8, 4);

        vt.feed_str("abcd");
        vt.feed_str("\x1b[B");

        assert_eq!(text(&vt), "abcd\n    |\n\n");

        vt.feed_str("\x1b[2B");

        assert_eq!(text(&vt), "abcd\n\n\n    |");
    }

    #[test]
    fn execute_cuf() {
        let mut vt = Vt::new(4, 1);

        vt.feed_str("\x1b[2C");

        assert_eq!(text(&vt), "  |");

        vt.feed_str("\x1b[2C");

        assert_eq!(text(&vt), "   |");

        vt.feed_str("a");

        assert_eq!(text(&vt), "   a|");

        vt.feed_str("\x1b[5C");

        assert_eq!(text(&vt), "   |a");

        vt.feed_str("ab");
        vt.feed_str("\x1b[10C");

        assert_eq!(text(&vt), "b  |");
    }

    #[test]
    fn execute_cub() {
        let mut vt = Vt::new(8, 2);

        vt.feed_str("abcd");
        vt.feed_str("\x1b[2D");

        assert_eq!(text(&vt), "ab|cd\n");

        vt.feed_str("cdef");
        vt.feed_str("\x1b[2D");

        assert_eq!(text(&vt), "abcd|ef\n");

        vt.feed_str("\x1b[10D");

        assert_eq!(text(&vt), "|abcdef\n");

        let mut vt = Vt::new(4, 2);

        vt.feed_str("abcd");
        vt.feed_str("\x1b[D");

        assert_eq!(text(&vt), "ab|cd\n");
    }

    #[test]
    fn execute_ich() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijklmn");

        vt.feed_str("\x1b[@");

        assert_eq!(text(&vt), "abc| defg\nijklmn");
        assert_eq!(wrapped(&vt), vec![true, false]);

        vt.feed_str("\x1b[2@");

        assert_eq!(text(&vt), "abc|   de\nijklmn");

        vt.feed_str("\x1b[10@");

        assert_eq!(text(&vt), "abc|\nijklmn");

        let mut vt = build_vt(8, 2, 7, 0, "abcdefghijklmn");

        vt.feed_str("\x1b[10@");
        assert_eq!(text(&vt), "abcdefg|\nijklmn");
    }

    #[test]
    fn execute_il() {
        let mut vt = build_vt(4, 4, 2, 1, "abcdefghij");

        vt.feed_str("\x1b[L");

        assert_eq!(text(&vt), "abcd\n  |\nefgh\nij");
        assert_eq!(wrapped(&vt), vec![false, false, true, false]);

        vt.feed_str("\x1b[A");
        vt.feed_str("\x1b[L");

        assert_eq!(text(&vt), "  |\nabcd\n\nefgh");
        assert_eq!(wrapped(&vt), vec![false, false, false, false]);

        vt.feed_str("\x1b[3B");
        vt.feed_str("\x1b[100L");

        assert_eq!(text(&vt), "\nabcd\n\n  |");
    }

    #[test]
    fn execute_dl() {
        let mut vt = Vt::new(4, 4);

        vt.feed_str("abcdefghijklmn");
        vt.feed_str("\x1b[2A");
        vt.feed_str("\x1b[M");

        assert_eq!(text(&vt), "abcd\nij|kl\nmn\n");
        assert_eq!(wrapped(&vt), vec![false, true, false, false]);

        // cursor above bottom margin

        let mut vt = Vt::new(4, 4);

        vt.feed_str("abcdefghijklmn");
        vt.feed_str("\x1b[1;3r");
        vt.feed_str("\x1b[2;1H");
        vt.feed_str("\x1b[M");

        assert_eq!(text(&vt), "abcd\n|ijkl\n\nmn");
        assert_eq!(wrapped(&vt), vec![false, false, false, false]);

        // cursor below bottom margin

        let mut vt = Vt::new(4, 4);

        vt.feed_str("abcdefghijklmn");
        vt.feed_str("\x1b[1;2r");
        vt.feed_str("\x1b[4;1H");
        vt.feed_str("\x1b[M");

        assert_eq!(text(&vt), "abcd\nefgh\nijkl\n|");
        assert_eq!(wrapped(&vt), vec![true, true, false, false]);
    }

    #[test]
    fn execute_el() {
        // short lines

        // a) clear to the end of the line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");

        vt.feed_str("\x1b[0K");

        assert_eq!(text(&vt), "ab|\n");

        let mut vt = build_vt(4, 2, 2, 0, "a");

        vt.feed_str("\x1b[0K");

        assert_eq!(text(&vt), "a |\n");

        // b) clear to the beginning of the line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");

        vt.feed_str("\x1b[1K");

        assert_eq!(text(&vt), "  | d\n");

        // c) clear the whole line

        let mut vt = build_vt(4, 2, 2, 0, "abcd");

        vt.feed_str("\x1b[2K");

        assert_eq!(text(&vt), "  |\n");

        // wrapped lines

        // a) clear to the end of the line

        let mut vt = Vt::new(4, 3);

        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[0K");

        assert_eq!(text(&vt), "abcd\nef|\nij");
        assert_eq!(wrapped(&vt), vec![true, false, false]);

        // b) clear to the beginning of the line

        let mut vt = Vt::new(4, 3);

        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[1K");

        assert_eq!(text(&vt), "abcd\n  | h\nij");
        assert_eq!(wrapped(&vt), vec![true, true, false]);

        // c) clear the whole line

        let mut vt = Vt::new(4, 3);

        vt.feed_str("abcdefghij\x1b[A");
        vt.feed_str("\x1b[2K");

        assert_eq!(text(&vt), "abcd\n  |\nij");
        assert_eq!(wrapped(&vt), vec![true, false, false]);
    }

    #[test]
    fn execute_ed() {
        // short lines

        // a) clear to the end of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        vt.feed_str("\x1b[0J");

        assert_eq!(text(&vt), "abc\nd|\n");

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\n\r\nghi");

        vt.feed_str("\x1b[0J");

        assert_eq!(text(&vt), "abc\n |\n");

        // b) clear to the beginning of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        vt.feed_str("\x1b[1J");

        assert_eq!(text(&vt), "\n | f\nghi");

        // c) clear the whole screen

        let mut vt = build_vt(4, 3, 1, 1, "abc\r\ndef\r\nghi");

        vt.feed_str("\x1b[2J");

        assert_eq!(text(&vt), "\n |\n");

        // wrapped lines

        // a) clear to the end of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");

        vt.feed_str("\x1b[0J");

        assert_eq!(text(&vt), "abcd\ne|\n");
        assert_eq!(wrapped(&vt), vec![true, false, false]);

        // b) clear to the beginning of the screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");

        vt.feed_str("\x1b[1J");

        assert_eq!(text(&vt), "\n | gh\nij");
        assert_eq!(wrapped(&vt), vec![false, true, false]);

        // c) clear the whole screen

        let mut vt = build_vt(4, 3, 1, 1, "abcdefghij");

        vt.feed_str("\x1b[2J");

        assert_eq!(text(&vt), "\n |\n");
        assert_eq!(wrapped(&vt), vec![false, false, false]);
    }

    #[test]
    fn execute_dch() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijkl");

        vt.feed_str("\x1b[P");

        assert_eq!(text(&vt), "abc|efgh\nijkl");
        assert_eq!(wrapped(&vt), vec![false, false]);

        vt.feed_str("\x1b[2P");

        assert_eq!(text(&vt), "abc|gh\nijkl");

        vt.feed_str("\x1b[10P");

        assert_eq!(text(&vt), "abc|\nijkl");

        vt.feed_str("\x1b[10C");
        vt.feed_str("\x1b[10P");

        assert_eq!(text(&vt), "abc    |\nijkl");
    }

    #[test]
    fn execute_ech() {
        let mut vt = build_vt(8, 2, 3, 0, "abcdefghijkl");

        vt.feed_str("\x1b[X");

        assert_eq!(text(&vt), "abc| efgh\nijkl");
        assert_eq!(wrapped(&vt), vec![true, false]);

        vt.feed_str("\x1b[2X");

        assert_eq!(text(&vt), "abc|  fgh\nijkl");
        assert_eq!(wrapped(&vt), vec![true, false]);

        vt.feed_str("\x1b[10X");

        assert_eq!(text(&vt), "abc|\nijkl");
        assert_eq!(wrapped(&vt), vec![false, false]);

        vt.feed_str("\x1b[3C\x1b[X");

        assert_eq!(text(&vt), "abc   |\nijkl");
    }

    #[test]
    fn execute_cht() {
        let mut vt = build_vt(28, 1, 3, 0, "abcdefghijklmnopqrstuwxyzabc");

        vt.feed_str("\x1b[I");

        assert_eq!(vt.cursor(), (8, 0));

        vt.feed_str("\x1b[2I");

        assert_eq!(vt.cursor(), (24, 0));

        vt.feed_str("\x1b[I");

        assert_eq!(vt.cursor(), (27, 0));
    }

    #[test]
    fn execute_cbt() {
        let mut vt = build_vt(28, 1, 26, 0, "abcdefghijklmnopqrstuwxyzabc");

        vt.feed_str("\x1b[Z");

        assert_eq!(vt.cursor(), (24, 0));

        vt.feed_str("\x1b[2Z");

        assert_eq!(vt.cursor(), (8, 0));

        vt.feed_str("\x1b[Z");

        assert_eq!(vt.cursor(), (0, 0));
    }

    #[test]
    fn execute_sc_rc() {
        // DECSC/DECRC variant

        let mut vt = build_vt(4, 3, 0, 0, "");

        // move 2x right, 1 down
        vt.feed_str("  \n");

        // save cursor
        vt.feed_str("\x1b7");

        // move 1x right, 1x down
        vt.feed_str(" \n");

        // restore cursor
        vt.feed_str("\x1b8");

        assert_eq!(vt.cursor(), (2, 1));

        // ansi.sys variant

        let mut vt = build_vt(4, 3, 0, 0, "");

        // move 2x right, 1 down
        vt.feed_str("  \n");

        // save cursor
        vt.feed_str("\x1b[s");

        // move 1x right, 1x down
        vt.feed_str(" \n");

        // restore cursor
        vt.feed_str("\x1b[u");

        assert_eq!(vt.cursor(), (2, 1));
    }

    #[test]
    fn execute_rep() {
        let mut vt = build_vt(20, 2, 0, 0, "");

        vt.feed_str("\x1b[b"); // REP

        assert_eq!(text(&vt), "|\n");

        vt.feed_str("A");
        vt.feed_str("\x1b[b");

        assert_eq!(text(&vt), "AA|\n");

        vt.feed_str("\x1b[3b");

        assert_eq!(text(&vt), "AAAAA|\n");

        vt.feed_str("\x1b[5C"); // move 5 cols to the right
        vt.feed_str("\x1b[b");

        assert_eq!(text(&vt), "AAAAA      |\n");
    }

    #[test]
    fn execute_xtwinops_wider() {
        let mut builder = Vt::builder();
        builder.resizable(true);

        let mut vt = builder.size(6, 6).build();

        vt.feed_str("\x1b[8;6;7t");

        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.view().iter().any(|l| l.wrapped));

        vt.feed_str("\x1b[8;6;15t");

        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.view().iter().any(|l| l.wrapped));

        let mut vt = builder.size(6, 6).build();

        vt.feed_str("000000111111222222333333444444555");

        assert_eq!(text(&vt), "000000\n111111\n222222\n333333\n444444\n555|");
        assert_eq!(wrapped(&vt), vec![true, true, true, true, true, false]);

        vt.feed_str("\x1b[8;6;7t");

        assert_eq!(text(&vt), "0000001\n1111122\n2222333\n3334444\n44555|\n");
        assert_eq!(wrapped(&vt), vec![true, true, true, true, false, false]);

        vt.feed_str("\x1b[8;6;15t");

        assert_eq!(text(&vt), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        let mut vt = builder.size(4, 3).build();

        vt.feed_str("000011\r\n22");

        assert_eq!(text(&vt), "0000\n11\n22|");
        assert_eq!(wrapped(&vt), vec![true, false, false]);

        vt.feed_str("\x1b[8;3;8t");

        assert_eq!(text(&vt), "000011\n22|\n");
        assert_eq!(wrapped(&vt), vec![false, false, false]);
    }

    #[test]
    fn execute_xtwinops_narrower() {
        let mut builder = Vt::builder();
        builder.resizable(true);

        let mut vt = builder.size(15, 6).build();

        vt.feed_str("\x1b[8;6;7t");

        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.view().iter().any(|l| l.wrapped));

        vt.feed_str("\x1b[8;6;6t");

        assert_eq!(text(&vt), "|\n\n\n\n\n");
        assert!(!vt.view().iter().any(|l| l.wrapped));

        let mut vt = builder.size(8, 2).build();

        vt.feed_str("\nabcdef");

        assert_eq!(wrapped(&vt), vec![false, false]);

        vt.feed_str("\x1b[8;;4t");

        assert_eq!(text(&vt), "abcd\nef|");
        assert_eq!(wrapped(&vt), vec![true, false]);

        let mut vt = builder.size(15, 6).build();

        vt.feed_str("000000111111222222333333444444555");

        assert_eq!(text(&vt), "000000111111222\n222333333444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        vt.feed_str("\x1b[8;6;7t");

        assert_eq!(text(&vt), "2222333\n3334444\n44555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);

        vt.feed_str("\x1b[8;6;6t");

        assert_eq!(text(&vt), "333333\n444444\n555|\n\n\n");
        assert_eq!(wrapped(&vt), vec![true, true, false, false, false, false]);
    }

    #[test]
    fn execute_xtwinops() {
        let mut vt = Vt::builder().size(8, 4).resizable(true).build();
        vt.feed_str("abcdefgh\r\nijklmnop\r\nqrstuw");
        vt.feed_str("\x1b[4;1H");

        let (_, resized, _) = vt.feed_str("AAA");

        assert!(!resized);

        let (_, resized, _) = vt.feed_str("\x1b[8;5;t");

        assert!(resized);
        assert_eq!(text(&vt), "abcdefgh\nijklmnop\nqrstuw\nAAA|\n");

        vt.feed_str("BBBBB");

        assert_eq!(vt.cursor(), (8, 3));

        let (_, resized, _) = vt.feed_str("\x1b[8;;4t");

        assert!(resized);
        assert_eq!(text(&vt), "qrst\nuw\nAAAB\nBBB|B\n");

        vt.feed_str("\rCCC");

        assert_eq!(text(&vt), "qrst\nuw\nAAAB\nCCC|B\n");
        assert_eq!(wrapped(&vt), vec![true, false, true, false, false]);

        vt.feed_str("\x1b[8;;3t");

        assert_eq!(text(&vt), "tuw\nAAA\nBCC\nC|B\n");

        vt.feed_str("\x1b[8;;5t");

        assert_eq!(text(&vt), "qrstu\nw\nAAABC\nCC|B\n");

        vt.feed_str("DDD");
        vt.feed_str("\x1b[8;;6t");

        assert_eq!(text(&vt), "op\nqrstuw\nAAABCC\nCDDD|\n");
    }

    #[test]
    fn execute_xtwinops_noop() {
        let mut vt = Vt::new(8, 4);

        let (_, resized, _) = vt.feed_str("\x1b[8;;t");

        assert!(!resized);
    }

    #[test]
    fn execute_xtwinops_taller() {
        let mut vt = Vt::builder().size(6, 4).resizable(true).build();

        vt.feed_str("AAA\n\rBBB\n\r");
        let (_, resized, _) = vt.feed_str("\x1b[8;5;;t");

        assert!(resized);
        assert_eq!(text(&vt), "AAA\nBBB\n|\n\n");
    }

    #[test]
    fn execute_xtwinops_shorter() {
        let mut vt = Vt::builder().size(6, 6).resizable(true).build();

        vt.feed_str("AAA\n\rBBB\n\rCCC\n\r");

        let (_, resized, _) = vt.feed_str("\x1b[8;5;;t");

        assert!(resized);
        assert_eq!(text(&vt), "AAA\nBBB\nCCC\n|\n");

        let (_, resized, _) = vt.feed_str("\x1b[8;3;;t");

        assert!(resized);
        assert_eq!(text(&vt), "BBB\nCCC\n|");

        let (_, resized, _) = vt.feed_str("\x1b[8;2;;t");

        assert!(resized);
        assert_eq!(text(&vt), "CCC\n|");
    }

    #[test]
    fn execute_xtwinops_vs_buffer_switching() {
        let mut vt = Vt::builder().size(4, 4).resizable(true).build();

        // fill primary buffer
        vt.feed_str("aaa\n\rbbb\n\rc\n\rddd");

        assert_eq!(vt.cursor(), (3, 3));

        // resize to 4x5
        vt.feed_str("\x1b[8;5;4;t");

        assert_eq!(text(&vt), "aaa\nbbb\nc\nddd|\n");

        // switch to alternate buffer
        vt.feed_str("\x1b[?1049h");

        assert_eq!(vt.cursor(), (3, 3));

        // resize to 4x2
        vt.feed_str("\x1b[8;2;4t");

        assert_eq!(vt.cursor(), (3, 1));

        // resize to 2x3, we'll check later if primary buffer preserved more columns
        vt.feed_str("\x1b[8;3;2t");

        // resize to 3x3
        vt.feed_str("\x1b[8;3;3t");

        // switch back to primary buffer
        vt.feed_str("\x1b[?1049l");

        assert_eq!(text(&vt), "bbb\nc\ndd|d");
    }

    #[test]
    fn dump_initial() {
        let vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_modified() {
        let mut vt1 = Vt::new(10, 4);
        let mut vt2 = Vt::new(10, 4);

        vt1.feed_str("hello\n\rworld\u{9b}5W\u{9b}7`\u{1b}[W\u{9b}?6h");
        vt1.feed_str("\u{9b}2;4r\u{9b}1;5H\x1b[1;31;41m\u{9b}?25l\u{9b}4h");
        vt1.feed_str("\u{9b}?7l\u{9b}20h\u{9b}\u{3a}\x1b(0\x1b)0\u{0e}");

        vt2.feed_str(&vt1.dump());

        assert_vts_eq(&vt1, &vt2);
    }

    #[test]
    fn dump_with_file() {
        if let Ok((w, h, input, step)) = setup_dump_with_file() {
            let mut vt1 = Vt::new(w, h);

            let mut s = 0;

            for c in input.chars().take(1_000_000) {
                vt1.feed(c);

                if s == 0 {
                    let d = vt1.dump();
                    let mut vt2 = Vt::new(w, h);

                    vt2.feed_str(&d);

                    assert_vts_eq(&vt1, &vt2);
                }

                s = (s + 1) % step;
            }
        }
    }

    #[test]
    fn charsets() {
        let mut vt = build_vt(6, 7, 0, 0, "");

        // GL points to G0, G0 is set to ascii
        vt.feed_str("alpty\r\n");

        // GL points to G0, G0 is set to drawing
        vt.feed_str("\x1b(0alpty\r\n");

        // GL points to G1, G1 is still set to ascii
        vt.feed_str("\u{0e}alpty\r\n");

        // GL points to G1, G1 is set to drawing
        vt.feed_str("\x1b)0alpty\r\n");

        // GL points to G1, G1 is set back to ascii
        vt.feed_str("\x1b)Balpty\r\n");

        // GL points to G0, G0 is set back to ascii
        vt.feed_str("\x1b(B\u{0f}alpty");

        assert_eq!(text(&vt), "alpty\n▒┌⎻├≤\nalpty\n▒┌⎻├≤\nalpty\nalpty|\n");
    }

    fn gen_input(max_len: usize) -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(
            prop_oneof![gen_ctl_seq(), gen_esc_seq(), gen_csi_seq(), gen_text()],
            1..=max_len,
        )
        .prop_map(flatten)
    }

    fn gen_ctl_seq() -> impl Strategy<Value = Vec<char>> {
        let ctl_chars = vec![0x00..0x18, 0x19..0x1a, 0x1c..0x20];

        prop::sample::select(flatten(ctl_chars)).prop_map(|v: u8| vec![v as char])
    }

    fn gen_esc_seq() -> impl Strategy<Value = Vec<char>> {
        (
            prop::collection::vec(gen_esc_intermediate(), 0..=2),
            gen_esc_finalizer(),
        )
            .prop_map(|(inters, fin)| flatten(vec![vec!['\x1b'], inters, vec![fin]]))
    }

    fn gen_csi_seq() -> impl Strategy<Value = Vec<char>> {
        prop_oneof![
            gen_csi_sgr_seq(),
            gen_csi_sm_seq(),
            gen_csi_rm_seq(),
            gen_csi_any_seq(),
        ]
    }

    fn gen_text() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(gen_char(), 1..10)
    }

    fn gen_esc_intermediate() -> impl Strategy<Value = char> {
        (0x20..0x30u8).prop_map(|v| v as char)
    }

    fn gen_esc_finalizer() -> impl Strategy<Value = char> {
        let finalizers = vec![
            0x30..0x50,
            0x51..0x58,
            0x59..0x5a,
            0x5a..0x5b,
            0x5c..0x5d,
            0x60..0x7f,
        ];

        prop::sample::select(flatten(finalizers)).prop_map(|v: u8| v as char)
    }

    fn gen_csi_sgr_seq() -> impl Strategy<Value = Vec<char>> {
        gen_csi_params().prop_map(|params| flatten(vec![vec!['\x1b', '['], params, vec!['m']]))
    }

    fn gen_csi_sm_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['h']])
        })
    }

    fn gen_csi_rm_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_intermediate(), gen_csi_sm_rm_param()).prop_map(|(inters, params)| {
            flatten(vec![vec!['\x1b', '['], inters, params, vec!['l']])
        })
    }

    fn gen_csi_any_seq() -> impl Strategy<Value = Vec<char>> {
        (gen_csi_params(), gen_csi_finalizer())
            .prop_map(|(params, fin)| flatten(vec![vec!['\x1b', '['], params, vec![fin]]))
    }

    fn gen_csi_intermediate() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(prop::sample::select(vec!['?', '!']), 0..=1)
    }

    fn gen_csi_params() -> impl Strategy<Value = Vec<char>> {
        prop::collection::vec(
            prop_oneof![
                gen_csi_param(),
                gen_csi_param(),
                prop::sample::select(vec![';'])
            ],
            0..=5,
        )
    }

    fn gen_csi_param() -> impl Strategy<Value = char> {
        (0x30..0x3au8).prop_map(|v| v as char)
    }

    fn gen_csi_sm_rm_param() -> impl Strategy<Value = Vec<char>> {
        let modes = vec![1, 4, 6, 7, 20, 25, 47, 1047, 1048, 1049];

        prop_oneof![
            prop::sample::select(modes).prop_map(|n| n.to_string().chars().collect()),
            prop::collection::vec(gen_csi_param(), 1..=4)
        ]
    }

    fn gen_csi_finalizer() -> impl Strategy<Value = char> {
        (0x40..0x7fu8).prop_map(|v| v as char)
    }

    fn gen_char() -> impl Strategy<Value = char> {
        prop_oneof![
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            gen_ascii_char(),
            (0x80..=0xd7ffu32).prop_map(|v| char::from_u32(v).unwrap()),
            (0xf900..=0xffffu32).prop_map(|v| char::from_u32(v).unwrap())
        ]
    }

    fn gen_ascii_char() -> impl Strategy<Value = char> {
        (0x20..=0x7fu8).prop_map(|v| v as char)
    }

    fn flatten<T, I: IntoIterator<Item = T>>(seqs: Vec<I>) -> Vec<T> {
        seqs.into_iter().flatten().collect()
    }

    proptest! {
        #[test]
        fn prop_sanity_checks_infinite_scrollback(input in gen_input(25)) {
            let mut vt = Vt::builder().size(10, 5).resizable(true).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().len() >= vt.size().1);
        }

        #[test]
        fn prop_sanity_checks_no_scrollback(input in gen_input(25)) {
            let mut vt = Vt::builder().size(10, 5).scrollback_limit(0).resizable(true).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().len() == vt.size().1);
        }

        #[test]
        fn prop_sanity_checks_fixed_scrollback(input in gen_input(25)) {
            let scrollback_limit = 3;
            let mut vt = Vt::builder().size(10, 5).scrollback_limit(scrollback_limit).resizable(true).build();

            vt.feed_str(&(input.into_iter().collect::<String>()));
            let (_, rows) = vt.size();

            vt.terminal.verify();
            assert!(vt.lines().len() >= rows && vt.lines().len() <= rows + scrollback_limit);
        }

        #[test]
        fn prop_resizing(new_cols in 2..15usize, new_rows in 2..8usize, input1 in gen_input(25), input2 in gen_input(25)) {
            let mut vt = Vt::builder().size(10, 5).resizable(true).build();

            vt.feed_str(&(input1.into_iter().collect::<String>()));
            vt.feed_str(&format!("\x1b[8;{};{}t", new_rows, new_cols));
            vt.feed_str(&(input2.into_iter().collect::<String>()));

            vt.terminal.verify();
            assert!(vt.lines().len() >= vt.size().1);
        }

        #[test]
        fn prop_dump(input in gen_input(25)) {
            let mut vt1 = Vt::new(10, 5);
            let mut vt2 = Vt::new(10, 5);

            vt1.feed_str(&(input.into_iter().collect::<String>()));
            vt2.feed_str(&vt1.dump());

            assert_vts_eq(&vt1, &vt2);
        }
    }

    fn setup_dump_with_file() -> Result<(usize, usize, String, usize), env::VarError> {
        let path = env::var("P")?;
        let input = fs::read_to_string(path).unwrap();
        let w: usize = env::var("W").unwrap().parse::<usize>().unwrap();
        let h: usize = env::var("H").unwrap().parse::<usize>().unwrap();
        let step: usize = env::var("S")
            .unwrap_or("1".to_owned())
            .parse::<usize>()
            .unwrap();

        Ok((w, h, input, step))
    }

    fn build_vt(cols: usize, rows: usize, cx: usize, cy: usize, init: &str) -> Vt {
        let mut vt = Vt::new(cols, rows);
        vt.feed_str(init);
        vt.feed_str(&format!("\u{9b}{};{}H", cy + 1, cx + 1));

        vt
    }

    fn assert_vts_eq(vt1: &Vt, vt2: &Vt) {
        vt1.parser.assert_eq(&vt2.parser);
        vt1.terminal.assert_eq(&vt2.terminal);
    }

    fn text(vt: &Vt) -> String {
        let cursor = vt.cursor();

        buffer_text(vt.terminal.view(), cursor.col, cursor.row)
    }

    fn buffer_text(view: &[Line], cursor_col: usize, cursor_row: usize) -> String {
        let mut lines = Vec::new();
        lines.extend(view[0..cursor_row].iter().map(|l| l.text()));
        let cursor_line = &view[cursor_row];
        let left = cursor_line.chars().take(cursor_col);
        let right = cursor_line.chars().skip(cursor_col);
        let mut line = String::from_iter(left);
        line.push('|');
        line.extend(right);
        lines.push(line);
        lines.extend(view[cursor_row + 1..].iter().map(|l| l.text()));

        lines
            .into_iter()
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wrapped(vt: &Vt) -> Vec<bool> {
        vt.terminal.view().iter().map(|l| l.wrapped).collect()
    }
}
