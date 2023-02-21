# asciinema virtual terminal

This repository contains the source code of virtual terminal emulator used
by [asciinema-player](https://github.com/asciinema/asciinema-player) and
[asciinema-server](https://github.com/asciinema/asciinema-server).

The emulator is based on
[Paul Williams' parser for ANSI-compatible video terminals](https://www.vt100.net/emu/dec_ansi_parser).
It covers only the display part of the emulation as only this is needed
by asciinema. Handling of escape sequences is fully compatible
with most modern terminal emulators like xterm, Gnome Terminal, iTerm, mosh etc.

## License

© 2019 Marcin Kulik.

All code is licensed under the Apache License, Version 2.0. See LICENSE file for details.
