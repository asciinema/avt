# avt - asciinema virtual terminal

avt is asciinema's implementation of virtual terminal emulator written in Rust.
It's used by [asciinema player](https://github.com/asciinema/asciinema-player),
[asciinema server](https://github.com/asciinema/asciinema-server) and [asciinema
gif generator](https://github.com/asciinema/agg).

This implementation covers only parsing and virtual buffer related aspects of a
terminal emulator as it's all asciinema needs.

avt consists of:

- parser for ANSI-compatible video terminal based on [excellent state diagram by Paul Williams](https://www.vt100.net/emu/dec_ansi_parser),
- virtual screen buffers (primary/alternate) in a form of character grid with additional color/styling attributes,
- API for feeding text into the parser and for querying virtual screen buffer and cursor position.

Following aspects of terminal emulation are not in scope of this project:

- input handling,
- scrollback,
- rendering.

Therefore avt is small and focused but a full-fledged terminal emulator can't be
built with it.

avt doesn't try to replicate a specific terminal variant like VT102 or VT220 and
instead implements most control sequences found in modern terminal emulators
like xterm, Gnome Terminal, Alacritty, iTerm, mosh etc.

## License

© 2019 Marcin Kulik.

All code is licensed under the Apache License, Version 2.0. See LICENSE file for details.
