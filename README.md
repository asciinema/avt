# avt - asciinema virtual terminal

[![Test](https://github.com/asciinema/avt/actions/workflows/test.yml/badge.svg)](https://github.com/asciinema/avt/actions/workflows/test.yml)
[![Crates.io](https://img.shields.io/crates/v/avt.svg)](https://crates.io/crates/avt)

avt is asciinema's implementation of virtual terminal emulator written in Rust.

It is used by [asciinema CLI](https://github.com/asciinema/asciinema),
[asciinema player](https://github.com/asciinema/asciinema-player), [asciinema
server](https://github.com/asciinema/asciinema-server) and [asciinema gif
generator](https://github.com/asciinema/agg).

avt parses text and control sequences, maintains virtual screen state, and
provides synchronous observers of terminal operations.

avt consists of:

- parser for ANSI-compatible video terminal based on [excellent state diagram by Paul Williams](https://www.vt100.net/emu/dec_ansi_parser),
- virtual screen buffers (primary/alternate) in a form of character grid with additional color/styling attributes,
- API for feeding text or parsed actions and querying screen buffers and cursor position,
- composable observers for dirty rows, primary-text transcripts, and OSC 133 shell integration.

avt could serve as the emulation component of a full terminal emulator, but the
following are outside its scope:

- user input handling, such as keyboard and mouse events,
- rendering (see [asciinema player](https://github.com/asciinema/asciinema-player) and [agg](https://github.com/asciinema/agg)).

avt doesn't try to 100% replicate any specific terminal variant like VT102 or VT520,
instead it implements most control sequences supported by modern terminal emulators
like Gnome Console, WezTerm, Alacritty, iTerm2, Ghostty, mosh etc.

## Usage

Requires Rust 1.86 or later:

```toml
[dependencies]
avt = "0.19"
```

```rust
use avt::Terminal;

fn main() -> Result<(), avt::InvalidSize> {
    let mut terminal = Terminal::new(80, 24)?;
    terminal.feed("hello\r\nworld");

    let line = terminal.screen().line(1).unwrap();

    assert_eq!(line.text().trim_end(), "world");
    assert_eq!(terminal.cursor().position(), Some((5, 1)));

    Ok(())
}
```

`Terminal::new` retains no scrollback. Use
`Terminal::with_scrollback(80, 24, Some(1_000))` for bounded primary history, or
`None` for unlimited retention. Both constructors reject zero dimensions.
Caller-requested resizes clamp dimensions to at least one.

Plain `feed`, `execute`, and `resize` return `()`. Their `_with` variants return
the observer's report. Feed accepts strings, characters, and flat iterables;
`Parser` can also parse independently for `Terminal::execute`.

For incremental rendering, use `DirtyRows` to identify rows to repaint:

```rust
use avt::observer::DirtyRows;
use avt::Terminal;

fn main() -> Result<(), avt::InvalidSize> {
    let mut terminal = Terminal::new(80, 24)?;
    let mut dirty = DirtyRows::new();
    let rows = terminal.feed_with("hello\r\nworld", &mut dirty);

    for row in rows {
        let line = terminal.screen().line(row).unwrap();
        // Render line.cells(), or inspect line.text().
    }

    let cursor = terminal.cursor().position();

    Ok(())
}
```

Keep observers across calls and pass them through every mutation, including
resizes. `DirtyRows` reports a full repaint on its first call; cursor-only changes
must be tracked separately. Observers compose as tuples:

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

`Transcript` collects plain-text output from the primary screen, joining
soft-wrapped lines and preserving text as it scrolls out of view. Call `finish`
to collect the remaining content. Alternate-screen content is excluded.

avt is still pre-1.0. The API is settling, but breaking changes remain
possible. See the [migration guide](MIGRATING.md) if you're migrating from an
earlier version.

### Examples

Run an example with `cargo run --example NAME`:

| Example | Purpose |
| --- | --- |
| `text` | Read text and control sequences from stdin and print a transcript |
| `render` | Repaint dirty rows and cursor positions |
| `dump_restore` | Initialize a second terminal from a state dump and continue processing input |
| `transcript` | Preserve primary text across scrolling, reflow, and clears |
| `history` | Reconstruct submitted commands from OSC 133 markers |
| `prompt_markers` | Locate prompt annotations and track their outcomes after reflow |
| `command_timestamps` | Associate submitted commands with application-provided prompt-start times |
| `index_text` | Compose a custom primary-text indexer with shell tracking |
| `slice` | Keep operations needed to reproduce selected screen cells and the cursor |

## Development

To work on avt, first download the source code:

```sh
git clone https://github.com/asciinema/avt
cd avt
```

The recommended way to get the toolchain is the Nix dev shell, which provides
everything needed and just works:

```sh
nix develop
```

If you don't use Nix, you need the Rust toolchain (1.86 or later) with Cargo,
installed via your system package manager or [rustup](https://rustup.rs/).

Then build the library with:

```sh
cargo build
```

To run the test suite:

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
```

To run the benchmarks:

```sh
cargo bench
```

For benchmark smoke checks without timing, use `cargo bench --bench NAME -- --test`
with `parser`, `terminal`, or `charset`.

`./fuzz.sh` repeatedly runs property tests.

If you'd like to propose or submit any changes, please read the
[contribution guidelines](CONTRIBUTING.md) first.

## Donations

Sustainability of asciinema development relies on donations and sponsorships.

Please help the software project you use and love. Become a
[supporter](https://docs.asciinema.org/donations/#individuals) or a [corporate
sponsor](https://docs.asciinema.org/donations/#corporate-sponsorship).

asciinema is sponsored by:

- [Brightbox](https://www.brightbox.com/)

## Consulting

If you're interested in customization of avt or any other asciinema component
to suit your corporate needs, check [asciinema consulting
services](https://docs.asciinema.org/consulting/).

## License

© 2019 Marcin Kulik.

All code is licensed under the Apache License, Version 2.0. See LICENSE file for details.
