use crate::line::Line;
use crate::vt::Vt;
use std::mem;

#[derive(Default)]
pub struct TextUnwrapper {
    wrapped_line: String,
}

impl TextUnwrapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, line: &Line) -> Option<String> {
        if line.wrapped {
            self.wrapped_line.push_str(&line.text());

            None
        } else {
            self.wrapped_line.push_str(line.text().trim_end());

            Some(mem::take(&mut self.wrapped_line))
        }
    }

    pub fn flush(self) -> Option<String> {
        if self.wrapped_line.is_empty() {
            None
        } else {
            Some(self.wrapped_line)
        }
    }
}

pub struct TextCollector {
    vt: Vt,
    unwrapper: TextUnwrapper,
}

impl TextCollector {
    pub fn new(vt: Vt) -> Self {
        Self {
            vt,
            unwrapper: TextUnwrapper::new(),
        }
    }

    pub fn feed_str(&mut self, s: &str) -> impl Iterator<Item = String> + '_ {
        self.vt
            .feed_str(s)
            .scrollback
            .filter_map(|l| self.unwrapper.push(&l))
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> impl Iterator<Item = String> + '_ {
        self.vt
            .feed_str(&format!("\x1b[8;{rows};{cols}t"))
            .scrollback
            .filter_map(|l| self.unwrapper.push(&l))
    }

    pub fn flush(self) -> Vec<String> {
        let mut unwrapper = self.unwrapper;

        let mut lines: Vec<String> = self
            .vt
            .lines()
            .iter()
            .filter_map(|l| unwrapper.push(l))
            .collect();

        lines.extend(unwrapper.flush());

        while !lines.is_empty() && lines[lines.len() - 1].is_empty() {
            lines.truncate(lines.len() - 1);
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::TextUnwrapper;
    use crate::{util::TextCollector, Line, Pen, Vt};

    #[test]
    fn text_unwrapper() {
        let mut tu = TextUnwrapper::new();
        let pen = Pen::default();

        let mut line = Line::blank(5, pen);
        line.print(0, 'a'.into());
        line.print(4, 'b'.into());
        line.wrapped = false;

        let text = tu.push(&line);

        assert!(matches!(text, Some(ref x) if x == "a   b"));

        let mut line = Line::blank(5, pen);
        line.print(0, 'c'.into());
        line.print(4, 'd'.into());
        line.wrapped = true;

        let text = tu.push(&line);

        assert!(text.is_none());

        let mut line = Line::blank(5, pen);
        line.print(0, 'e'.into());
        line.print(4, 'f'.into());
        line.wrapped = true;

        let text = tu.push(&line);

        assert!(text.is_none());

        let mut line = Line::blank(5, pen);
        line.print(0, 'g'.into());
        line.print(1, 'h'.into());
        line.wrapped = false;

        let text = tu.push(&line);

        assert!(matches!(text, Some(ref x) if x == "c   de   fgh"));

        let mut line = Line::blank(5, pen);
        line.print(0, 'i'.into());
        line.wrapped = true;

        let text = tu.push(&line);

        assert!(text.is_none());

        let text = tu.flush();

        assert!(matches!(text, Some(ref x) if x == "i    "));
    }

    #[test]
    fn text_collector_no_scrollback() {
        let vt = Vt::builder().size(10, 2).scrollback_limit(0).build();
        let mut tc = TextCollector::new(vt);

        let lines: Vec<String> = tc.feed_str("a\r\nb\r\nc\r\nd\r\n").collect();

        assert_eq!(lines, ["a", "b", "c"]);

        let lines: Vec<String> = tc.flush();

        assert_eq!(lines, ["d"]);
    }

    #[test]
    fn text_collector_unlimited_scrollback() {
        let vt = Vt::builder().size(10, 2).build();
        let mut tc = TextCollector::new(vt);

        let lines: Vec<String> = tc.feed_str("a\r\nb\r\nc\r\nd\r\n").collect();

        assert!(lines.is_empty());

        let lines: Vec<String> = tc.flush();

        assert_eq!(lines, ["a", "b", "c", "d"]);
    }

    #[test]
    fn text_collector_wrapping() {
        let vt = Vt::builder().size(10, 2).scrollback_limit(0).build();
        let mut tc = TextCollector::new(vt);

        let lines: Vec<String> = tc.feed_str("abcdefghijklmno\r\n").collect();

        assert!(lines.is_empty());

        let lines: Vec<String> = tc.flush();

        assert_eq!(lines, vec!["abcdefghijklmno"]);
    }
}
