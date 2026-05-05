# avt - asciinema virtual terminal

[![Test](https://github.com/asciinema/avt/actions/workflows/test.yml/badge.svg)](https://github.com/asciinema/avt/actions/workflows/test.yml)
[![Crates.io](https://img.shields.io/crates/v/avt.svg)](https://crates.io/crates/avt)

avt is asciinema's implementation of virtual terminal emulator written in Rust.

It is used by [asciinema CLI](https://github.com/asciinema/asciinema),
[asciinema player](https://github.com/asciinema/asciinema-player), [asciinema
server](https://github.com/asciinema/asciinema-server) and [asciinema gif
generator](https://github.com/asciinema/agg).

This implementation covers only parsing and virtual buffer related aspects of a
terminal emulator as it's all asciinema needs.

avt consists of:

- parser for ANSI-compatible video terminal based on [excellent state diagram by Paul Williams](https://www.vt100.net/emu/dec_ansi_parser),
- virtual screen buffers (primary/alternate) in a form of character grid with additional color/styling attributes,
- API for feeding text into the parser and for querying virtual screen buffer and cursor position.

Following aspects of terminal emulation are not in scope of this project:

- input handling,
- rendering.

While avt is small and focused, a full-fledged terminal emulator could potentially be
built on top of it.

avt doesn't try to 100% replicate any specific terminal variant like VT102 or VT520,
instead it implements most control sequences supported by modern terminal emulators
like xterm, Gnome Terminal, WezTerm, Alacritty, iTerm, Ghostty, mosh etc.

## Building

Building avt from source requires the [Rust](https://www.rust-lang.org/)
compiler (1.82 or later) and the [Cargo package
manager](https://doc.rust-lang.org/cargo/). If they are not available via your
system package manager then use [rustup](https://rustup.rs/).

To download the source code and build the library run:

```sh
git clone https://github.com/asciinema/avt
cd avt
cargo build --release
```

To run the test suite:

```sh
cargo test
```

To run the benchmarks:

```sh
cargo bench
```

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
