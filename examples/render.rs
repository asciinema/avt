//! Repaint changed rows, including the old and new cursor positions.
use avt::DirtyRows;
use avt::Terminal;

fn main() {
    let mut terminal = Terminal::new(16, 3).unwrap();
    let mut dirty = DirtyRows::new();

    for output in [
        "\x1b[31m界 hello\x1b[0m",
        "\r\nnext line",
        "\x1b[1;1H",
        "\x1b[?25l",
    ] {
        let old_cursor = terminal.cursor().position();
        let mut rows = terminal.feed_with(output, &mut dirty);
        let cursor = terminal.cursor().position();

        // DirtyRows requests a full repaint initially. Cursor-only changes
        // require repainting the cursor rows separately.
        rows.extend(old_cursor.map(|(_, row)| row));
        rows.extend(cursor.map(|(_, row)| row));
        rows.sort_unstable();
        rows.dedup();

        for row in rows {
            let line = terminal.screen().line(row).unwrap();
            println!("row {row}: {:?}", line.text());
            // A graphical renderer can read line.cells() for width and styling.
        }

        println!("cursor: {cursor:?}");
    }
}
