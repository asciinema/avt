use crate::buffer::ScrollbackCollector;
use crate::line::Line;
use crate::vt::Vt;
use std::convert::Infallible;
use std::mem;

pub struct TextCollector<O: TextCollectorOutput> {
    vt: Vt,
    stc: ScrollbackTextCollector<O>,
}

pub trait TextCollectorOutput {
    type Error;

    fn push(&mut self, line: String) -> Result<(), Self::Error>;
}

struct ScrollbackTextCollector<O: TextCollectorOutput> {
    wrapped_line: String,
    output: O,
}

impl<O: TextCollectorOutput> TextCollector<O> {
    pub fn new(vt: Vt, output: O) -> Self {
        Self {
            vt,
            stc: ScrollbackTextCollector {
                wrapped_line: String::new(),
                output,
            },
        }
    }
}

impl<O: TextCollectorOutput> TextCollector<O> {
    pub fn feed_str(&mut self, s: &str) -> Result<(), O::Error> {
        self.vt.feed_str_sc(s, &mut self.stc)?;

        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), O::Error> {
        self.vt
            .feed_str_sc(&format!("\x1b[8;{rows};{cols}t"), &mut self.stc)?;

        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), O::Error> {
        let mut lines = self.vt.text();

        while !lines.is_empty() && lines[lines.len() - 1].is_empty() {
            lines.truncate(lines.len() - 1);
        }

        for line in lines {
            self.stc.push(line)?;
        }

        Ok(())
    }
}

impl<O: TextCollectorOutput> ScrollbackTextCollector<O> {
    fn push(&mut self, line: String) -> Result<(), O::Error> {
        self.output.push(line)
    }
}

impl<O: TextCollectorOutput> ScrollbackCollector for &mut ScrollbackTextCollector<O> {
    type Error = O::Error;

    fn collect(&mut self, lines: impl Iterator<Item = Line>) -> Result<(), Self::Error> {
        for line in lines {
            if line.wrapped {
                self.wrapped_line.push_str(&line.text());
            } else {
                self.wrapped_line.push_str(line.text().trim_end());
                self.output.push(mem::take(&mut self.wrapped_line))?;
            }
        }

        Ok(())
    }
}

impl TextCollectorOutput for &mut Vec<String> {
    type Error = Infallible;

    fn push(&mut self, line: String) -> Result<(), Self::Error> {
        Vec::push(self, line);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TextCollector;
    use crate::Vt;

    #[test]
    fn text_collector_no_scrollback() {
        let mut output: Vec<String> = Vec::new();
        let vt = Vt::builder().size(10, 2).scrollback_limit(0).build();
        let mut tc = TextCollector::new(vt, &mut output);

        tc.feed_str("a\r\nb\r\nc\r\nd\r\n").unwrap();
        tc.flush().unwrap();

        assert_eq!(output, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn text_collector_unlimited_scrollback() {
        let mut output: Vec<String> = Vec::new();
        let vt = Vt::builder().size(10, 2).build();
        let mut tc = TextCollector::new(vt, &mut output);

        tc.feed_str("a\r\nb\r\nc\r\nd\r\n").unwrap();
        tc.flush().unwrap();

        assert_eq!(output, vec!["a", "b", "c", "d"]);
    }
}
