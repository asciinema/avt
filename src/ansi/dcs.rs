//! DCS command handling. The outer parser owns framing and reports how data ends.

use super::{CursorPresentation, Function, Param};
use alloc::string::String;

const CURSOR_REPORT_LIMIT: usize = 128;

pub(super) enum End {
    Escape,
    St,
    Cancel,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Handler {
    cursor_report: Option<String>,
}

impl Handler {
    pub(super) fn hook(&mut self, params: &[Param], intermediate: Option<char>, final_char: char) {
        self.cursor_report = (final_char == 't'
            && intermediate == Some('$')
            && matches!(params, [param] if param.as_u16() == 1))
        .then(String::new);
    }

    pub(super) fn put(&mut self, input: char) {
        let Some(payload) = &mut self.cursor_report else {
            return;
        };

        if input.is_ascii() && payload.len() < CURSOR_REPORT_LIMIT {
            payload.push(input);
        } else {
            self.cursor_report = None;
        }
    }

    pub(super) fn end(&mut self, reason: End) -> Option<Function<'static>> {
        let payload = self.cursor_report.take()?;

        match reason {
            End::Escape | End::St => parse_cursor_report(&payload).map(Function::Decrsps),
            End::Cancel => None,
        }
    }

    pub(super) fn dump(&self, seq: &mut String) {
        if let Some(payload) = &self.cursor_report {
            seq.push_str("\u{90}1$t");
            seq.push_str(payload);
        } else {
            // An unsupported DCS preserves ignored-payload framing.
            seq.push_str("\u{90}@");
        }
    }
}

fn parse_cursor_report(payload: &str) -> Option<CursorPresentation> {
    let (row, rest) = payload.split_once(';')?;
    let (col, rest) = rest.split_once(';')?;
    let flags = rest.strip_prefix("1;@;@;")?.strip_suffix(";0;2;@;BBBB")?;

    let (origin_mode, pending_wrap) = match flags {
        "@" => (false, false),
        "A" => (true, false),
        "H" => (false, true),
        "I" => (true, true),
        _ => return None,
    };

    Some(CursorPresentation {
        row: report_number(row)?,
        col: report_number(col)?,
        origin_mode,
        pending_wrap,
    })
}

fn report_number(field: &str) -> Option<u16> {
    if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    field.parse().ok()
}
