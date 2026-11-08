//! Repaint changed rows, including the old and new cursor positions.
use avt::Terminal;
use avt::observer::DirtyRows;

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
        let mut row_indices = terminal.feed_with(output, &mut dirty);
        let cursor = terminal.cursor().position();

        // DirtyRows requests a full repaint initially. Cursor-only changes
        // require repainting the cursor rows separately.
        row_indices.extend(old_cursor.map(|(_, row)| row));
        row_indices.extend(cursor.map(|(_, row)| row));
        row_indices.sort_unstable();
        row_indices.dedup();

        for row_index in row_indices {
            let row = terminal.screen().row(row_index).unwrap();
            println!("row {row_index}: {:?}", row.text());
            // A graphical renderer can read row.cells() for width and styling.
        }

        println!("cursor: {cursor:?}");
    }
}
