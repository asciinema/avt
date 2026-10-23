# Migrating between avt versions

avt is still pre-1.0. The API is settling, but breaking changes remain possible.

Each section describes changes from the preceding version. If you skip versions,
read the relevant sections oldest-first. Examples use the API of their section.
This guide includes Git-tagged versions as well as crates.io releases.

[0.19](#019) · [0.18](#018) · [0.17](#017) · [0.16](#016) ·
[0.15](#015) · [0.14](#014) · [0.13](#013) · [0.12](#012) ·
[0.11](#011) · [0.10](#010) · [0.9](#09) · [0.8](#08) ·
[0.7](#07) · [0.6](#06) · [0.5](#05) · [0.1–0.4](#0104-legacy-vt)

## 0.19

Requires **Rust 1.86**. Replace both `avt::Vt` and `avt::terminal::Terminal` with
`avt::Terminal`.

### Construction and screen access

| Previous API | Replacement |
| --- | --- |
| `Vt::new(cols, rows)` | `Terminal::new(cols, rows)?` |
| Builder with `.scrollback_limit(n)` | `Terminal::with_scrollback(cols, rows, Some(n))?` |
| `terminal::Terminal::new((cols, rows), limit)` | `Terminal::with_scrollback(cols, rows, limit)?` |
| Default unlimited history | `Terminal::with_scrollback(cols, rows, None)?` |
| `feed_str(text)` | `feed(text)` |
| `view()` | `screen().visible_lines()` |
| `lines()` | `screen().all_lines()` |
| `line(row)` | `screen().line(row)`, returning `Option<&Line>` |
| `text()` | `primary_screen().text()` |
| Cursor conversion to `Option<(usize, usize)>` | `cursor().position()` |
| `terminal::BufferType`, `active_buffer_type()` | `ScreenKind`, `active_screen_kind()` |

`Terminal::new` retains **no scrollback**. Constructors return `InvalidSize` for
zero dimensions; `resize` clamps dimensions to at least one.

### Change tracking and text collection

Plain `feed`, `execute`, and `resize` return `()`. Use their `_with` variants with
`avt::observer::DirtyRows` instead of `Changes.lines`, and `Transcript` instead of
`util::TextCollector` or `TextUnwrapper`:

```rust
use avt::observer::{DirtyRows, Transcript};
use avt::Terminal;

fn main() -> Result<(), avt::InvalidSize> {
    let mut terminal = Terminal::new(80, 24)?;
    let mut dirty = DirtyRows::new();
    let mut transcript = Transcript::new();
    let (rows, mut lines) = terminal.feed_with("hello\r\n", (&mut dirty, &mut transcript));
    lines.extend(transcript.finish(&terminal));

    Ok(())
}
```

Reuse observers for every mutation, including resizes. `DirtyRows` starts with a
full repaint; track cursor changes separately. `Transcript` collects primary-screen
text: consume each report and call `finish` at the end. For raw evicted lines,
replace `Changes.scrollback` or `gc()` with an `Observer::evicted` hook.

### Other changes

- `Color::RGB` holds `avt::Rgb` instead of `rgb::RGB8`.
  Construct colors with `Color::rgb(r, g, b)`.
- Import `Cursor` from the crate root. Tuple comparisons are removed; compare
  `(cursor.col, cursor.row)` instead.
- Replace `Cell::is_default()` with a comparison against `Cell::default()`.
- Low-level terminal users: rename `cursor_keys_app_mode()` to
  `cursor_key_app_mode()`. `dump()` now returns a string to pass to `feed()`.
- Parser consumers must handle the new `Function::Osc` variant for OSC 133 markers.
- ED2 (`\x1b[2J`) preserves occupied primary rows in scrollback (subject to the
  retention limit) and makes them available to `Transcript`.
- Resizing reflows the primary screen even while alternate is active, and clips or
  pads the alternate screen instead of reflowing it.

## 0.18

- `Cell::width()` returns `u8` instead of `usize`; use `usize::from(cell.width())`
  when needed for indexing. `Cell::set` is removed; obtain cells from terminal output.
- `Function::Sgr`, `Sm`/`Rm`, and `Decset`/`Decrst` use `SgrOps`, `AnsiModes`, and
  `DecModes` instead of vectors. Convert vectors with `.into()`, for example
  `Function::Sgr(vec![SgrOp::Reset].into())`. Public slice access is available in 0.19.
- The low-level `terminal::Terminal::dump()` returns `Vec<Function>` instead of a
  string. Replay those functions with `execute()`. `Vt::dump()` still returns a string.

## 0.17

Requires **Rust 1.82**, up from 1.70.

`Vt::view()` and `Vt::lines()` return iterators instead of slices. Remove `.iter()`
from iterator chains; use `line(row)` for visible-row access or collect the
iterator when you need an owned collection.

## 0.16

- Wide characters occupy two cells: a width-2 character followed by a width-0
  continuation. Render the character once and skip the continuation cell.
  `Line::chars()` and `text()` already omit continuation cells.
- `Cell::from(char)` is removed. Obtain cells from terminal output instead.

## 0.15

- Use `Vt::resize(cols, rows)` instead of feeding XTWINOPS resize sequences
  (`\x1b[8;{rows};{cols}t`), which are now ignored by the terminal.
- Remove `.resizable(...)` from builder chains and stop reading `Changes.resized`.
  Explicit `resize()` returns `Changes` with dirty rows and evicted scrollback.

**0.15.1:** `Line::dump()` is removed. Use `Vt::dump()` for a terminal snapshot,
or serialize individual lines yourself from their cells and pens.

## 0.14

- `Line::cells()` returns `&[Cell]` instead of an iterator of `(char, Pen)` pairs.
  Use `cell.char()` and `cell.pen()`; indexed cells also use these accessors.
- `Segment`, `Line::segments()`, and `Line::group()` are removed. Use
  `line.chunks(|a, b| a.pen() != b.pen())` for runs with matching styles. The
  predicate receives adjacent cells and returns `true` to start a new chunk;
  each chunk is a `Vec<Cell>`.
- Built-in Serde serialization is removed. Convert colors, pens, and cell content
  into your application's serializable types.

## 0.13

Requires **Rust 1.70**.

- `Vt::feed_str()` returns `Changes` instead of a tuple. Read `.lines` and
  `.resized`; consume `.scrollback` if you need evicted lines. Drop the report
  before borrowing the terminal again. `feed_str_sc()` is removed.
- Replace `TextCollector::new(vt, output)` with `TextCollector::new(vt)`.
  Consume the iterators returned by `feed_str()` and `resize()` instead of
  implementing `TextCollectorOutput`. `flush()` consumes the collector and
  returns the remaining `Vec<String>`.
- Replace `arrow_key_app_mode()` with `cursor_key_app_mode()`.
- Standalone parser users: `Executor` and `Parser::feed_str()` are removed.
  Use `input.chars().filter_map(|ch| parser.feed(ch))` to obtain functions.
  `ops::Operation` becomes `parser::Function`; numeric parameters use `u16`
  and modes, erasures, and SGR use typed enums instead of `Param` values.
  Update matches, including `Sc`/`Rc` → `Decsc`/`Decrc` and
  `PrvSm`/`PrvRm` → `Decset`/`Decrst`.

## 0.12

Standalone parser users must replace the individual `Executor` callbacks with
`execute(&mut self, op: avt::ops::Operation)` and match on the operation.
`parser::Param` moves to `ops::Param`; `parser::Params` is no longer public.

## 0.11

- For custom parser executors, `Params::iter()` and `as_slice()` now expose
  `Param` values instead of `u16`. Use `Param::as_slice()` to read the numeric
  parts of colon-separated parameters.
- Bounded scrollback is trimmed in batches and can temporarily exceed the
  configured limit.

**0.11.1:** No migration required.

## 0.10

- **0.10.0:** Serialized segments change from `[text, pen]` tuples to objects
  with `text`, `pen`, and `offset` fields. Update consumers of serialized output.
- **0.10.1:** Serialized offsets count display columns rather than characters.
  Segments also split when character widths differ, not just when styles differ.
- **0.10.2:** `Line::group()` predicates receive `(&char, usize)` instead of
  `&char`; change `|ch| ...` to `|ch, width| ...` (or `|ch, _| ...`). Serialized
  segments gain a `charWidth` field.
- **0.10.3:** No migration required.

## 0.9

No migration required. This is the first version published to crates.io as `avt`.

## 0.8

- **0.8.0:** Primary scrollback is retained without a limit by default. Use
  `Vt::with_scrollback_limit(cols, rows, Some(n))` to bound it, or `Some(0)` to
  retain no history. `lines()` returns a slice instead of an iterator; add
  `.iter()` when using iterator methods.
- **0.8.1:** No migration required.
- **0.8.2:**
  - `lines()` now includes scrollback; use `view()` for visible rows.
  - Replace the `cols` and `rows` fields with `size()`, and assignment to
    `resizable` with `resizable(bool)`. `Vt::state` is removed; use the standalone
    `parser::Parser` if you need parser-state access.
  - `cursor()` returns `(col, row, visible)` instead of `Option<(col, row)>`.
- **0.8.3:**
  - Replace `with_scrollback_limit(...)` with
    `Vt::builder().size(cols, rows).scrollback_limit(n).build()`.
    Omit `.scrollback_limit(...)` for unlimited history. Move `resizable(bool)`
    into the builder chain as well.
  - `cursor()` returns a struct with `col`, `row`, and `visible` fields.
    Use `.into()` when you need `Option<(usize, usize)>`.

## 0.7

- **0.7.0:** `feed_str()` returns `(dirty_rows, resized)` instead of just dirty
  rows. The `columns` field becomes `cols`. The root `State` type is no longer
  exported.
- **0.7.1:** No migration required.
- **0.7.2–0.7.3:** XTWINOPS resizing is disabled, with no public opt-in. If you
  rely on it, use 0.7.4 or later. In 0.7.3, `CSI e` correctly moves the cursor
  down instead of up.
- **0.7.4:** Set `vt.resizable = true` to enable XTWINOPS resizing. Resizing now
  reflows wrapped lines instead of simply clipping or padding them.

## 0.6

Replace `get_line(row)` with `line(row).segments()`, and `get_lines()` with
iteration over `lines()` and each line's `segments()`. `lines()` now yields
borrowed `Line` values instead of owned vectors of `(char, Pen)` pairs; use
`Line::cells()` for those pairs and `.collect()` when you need owned results.

## 0.5

- **0.5.0:** `Pen` fields become private and `Intensity` is no longer exported.
  Read styles through `foreground()`, `background()`, and `is_*()` accessors
  such as `is_bold()` and `is_faint()`.
- **0.5.1:** The project is renamed from `vt` to `avt`, and `VT` becomes `Vt`.
  Update Git dependency declarations and imports from `vt::VT` to `avt::Vt`.

## 0.1–0.4 (legacy `vt`)

These versions predate the `avt` name. Versions 0.1–0.3 are recorded in manifest
history but have no Git tags; the initial 0.1 development history is the baseline.

- **0.2:** No migration required.
- **0.3:** Replace `get_cursor()` with `cursor()`. Constructors now require
  nonzero columns and rows.
- **0.4:** Replace the public `Pen::bold` boolean with `Pen::intensity`, using
  `Intensity::Normal`, `Bold`, or `Faint`.
