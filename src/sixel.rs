//! Sixel graphics decoding and placement.
//!
//! Sixel images arrive in a DCS (Device Control String) of the form
//! `ESC P <params> q <data> ST`. The parser strips the envelope and hands the
//! terminal the `q` marker, the macro parameters, and the data; this module
//! decodes that data into a tightly packed RGBA buffer ([`Sixel`]) and the
//! terminal anchors it to a grid cell ([`Image`]).
//!
//! Each data character in `?`..=`~` encodes one column of six vertically
//! stacked pixels (subtracting `0x3f` yields a six-bit value whose least
//! significant bit is the topmost pixel). Bands of six pixels stack
//! top-to-bottom, separated by the graphics-newline command `-`.

use std::sync::Arc;

use rgb::{RGB8, RGBA8};

const TRANSPARENT: RGBA8 = RGBA8::new(0, 0, 0, 0);

// Upper bound on a decoded image's pixel count. A malformed raster declaration
// ("...;Ph;Pv) can claim an enormous canvas while sending almost no data; this
// cap keeps untrusted terminal output from overflowing the width*height
// multiply or trying to allocate a multi-gigabyte buffer. 16M px (~64 MiB of
// RGBA8) is far larger than any legitimate terminal sixel.
const MAX_PIXELS: usize = 16_000_000;

/// A decoded sixel image: a row-major, tightly packed RGBA pixel buffer.
#[derive(Debug, PartialEq)]
pub struct Sixel {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<RGBA8>,
}

/// A decoded sixel image anchored to a terminal cell. The pixel data is shared
/// (`Arc`) so the image can scroll and move between buffers cheaply.
///
/// When the terminal knows its cell pixel size, the image also tracks its cell
/// footprint (`cols` x `rows`) and which of those cells have been overwritten by
/// later cell content (the [`occluded`](Image::is_occluded) mask). A real
/// terminal paints sixel pixels once and lets subsequent text overwrite them;
/// the mask lets a renderer reproduce that by skipping occluded cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// Anchor cell column (top-left of the image), in view coordinates.
    pub col: usize,
    /// Anchor cell row, in view coordinates.
    pub row: usize,
    data: Arc<Sixel>,
    cols: usize,
    rows: usize,
    occluded: Vec<bool>,
}

impl Image {
    /// Create an image anchored at cell (`col`, `row`) from a row-major RGBA
    /// buffer of `width * height` pixels. The cell footprint is unknown, so no
    /// occlusion is tracked.
    pub fn new(col: usize, row: usize, width: usize, height: usize, pixels: Vec<RGBA8>) -> Self {
        Image {
            col,
            row,
            data: Arc::new(Sixel {
                width,
                height,
                pixels,
            }),
            cols: 0,
            rows: 0,
            occluded: Vec::new(),
        }
    }

    pub(crate) fn from_sixel(col: usize, row: usize, sixel: Sixel, cell_size: Option<(usize, usize)>) -> Self {
        let (cols, rows) = match cell_size {
            Some((cw, ch)) if cw > 0 && ch > 0 => (sixel.width.div_ceil(cw), sixel.height.div_ceil(ch)),
            _ => (0, 0),
        };

        Image {
            col,
            row,
            data: Arc::new(sixel),
            cols,
            rows,
            occluded: vec![false; cols * rows],
        }
    }

    /// Image width in pixels.
    pub fn width(&self) -> usize {
        self.data.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> usize {
        self.data.height
    }

    /// The row-major RGBA pixel buffer (`width * height` pixels). Pixels never
    /// written by the sixel stream are transparent.
    pub fn pixels(&self) -> &[RGBA8] {
        &self.data.pixels
    }

    /// The image's cell footprint width, or `0` if the cell size is unknown.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The image's cell footprint height, or `0` if the cell size is unknown.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Whether the footprint cell at offset (`dcol`, `drow`) from the anchor has
    /// been overwritten by later cell content (and so should hide the image
    /// there). Always `false` when the cell size is unknown.
    pub fn is_occluded(&self, dcol: usize, drow: usize) -> bool {
        dcol < self.cols && drow < self.rows && self.occluded[drow * self.cols + dcol]
    }

    /// Mark the footprint cell containing view position (`col`, `row`) occluded,
    /// because cell content was drawn there after the image was placed.
    pub(crate) fn occlude(&mut self, col: usize, row: usize) {
        if self.cols == 0
            || col < self.col
            || row < self.row
            || col >= self.col + self.cols
            || row >= self.row + self.rows
        {
            return;
        }

        let idx = (row - self.row) * self.cols + (col - self.col);
        self.occluded[idx] = true;
    }
}

/// Decode the data portion of a sixel DCS (everything after the `q` marker,
/// excluding the string terminator). Returns `None` for an empty image.
pub(crate) fn decode(data: &str) -> Option<Sixel> {
    let mut decoder = Decoder::new();
    decoder.run(&data.chars().collect::<Vec<_>>());
    decoder.into_sixel()
}

struct Decoder {
    palette: Vec<RGB8>,
    color: RGB8,
    x: usize,
    band: usize,
    rows: Vec<Vec<RGBA8>>,
    max_width: usize,
    max_height: usize,
    declared_width: usize,
    declared_height: usize,
}

impl Decoder {
    fn new() -> Self {
        let palette = default_palette();
        let color = palette[0];

        Self {
            palette,
            color,
            x: 0,
            band: 0,
            rows: Vec::new(),
            max_width: 0,
            max_height: 0,
            declared_width: 0,
            declared_height: 0,
        }
    }

    fn run(&mut self, chars: &[char]) {
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                '#' => i = self.handle_color(chars, i + 1),
                '"' => i = self.handle_raster(chars, i + 1),
                '!' => i = self.handle_repeat(chars, i + 1),
                '$' => {
                    self.x = 0;
                    i += 1;
                }
                '-' => {
                    self.x = 0;
                    self.band += 6;
                    i += 1;
                }
                c @ '?'..='~' => {
                    self.put_sixel(c as u8 - 0x3f, 1);
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }

    /// `#Pc` selects color register `Pc`; `#Pc;Pu;Px;Py;Pz` defines it. `Pu` is
    /// the color space: `2` for RGB, `1` for HLS, components scaled to 0..=100
    /// (hue 0..360).
    fn handle_color(&mut self, chars: &[char], start: usize) -> usize {
        let (pc, mut i) = parse_number(chars, start);

        if !matches!(chars.get(i), Some(';')) {
            self.color = self.palette.get(pc).copied().unwrap_or(self.palette[0]);
            return i;
        }

        let pu;
        let px;
        let py;
        let pz;
        (pu, i) = parse_number(chars, i + 1);
        (px, i) = parse_param(chars, i);
        (py, i) = parse_param(chars, i);
        (pz, i) = parse_param(chars, i);

        let color = match pu {
            1 => hls_to_rgb(px, py, pz),
            _ => rgb_from_percent(px, py, pz),
        };

        if pc < self.palette.len() {
            self.palette[pc] = color;
        }

        self.color = color;
        i
    }

    /// `"Pan;Pad;Ph;Pv` declares the pixel aspect ratio (ignored) and the
    /// raster width `Ph`/height `Pv`, which size the canvas even where the
    /// stream leaves trailing rows or columns blank.
    fn handle_raster(&mut self, chars: &[char], start: usize) -> usize {
        let mut i = start;
        let _pan;
        let _pad;
        let ph;
        let pv;
        (_pan, i) = parse_number(chars, i);
        (_pad, i) = parse_param(chars, i);
        (ph, i) = parse_param(chars, i);
        (pv, i) = parse_param(chars, i);

        self.declared_width = self.declared_width.max(ph);
        self.declared_height = self.declared_height.max(pv);
        i
    }

    /// `!Pn<c>` repeats the sixel data character `c` `Pn` times.
    fn handle_repeat(&mut self, chars: &[char], start: usize) -> usize {
        let (count, i) = parse_number(chars, start);

        match chars.get(i) {
            Some(&c @ '?'..='~') => {
                self.put_sixel(c as u8 - 0x3f, count.max(1));
                i + 1
            }
            _ => i,
        }
    }

    /// Paint `count` consecutive columns from one six-bit sixel value, the
    /// least-significant bit being the topmost pixel of the current band.
    fn put_sixel(&mut self, value: u8, count: usize) {
        for _ in 0..count {
            for bit in 0..6 {
                if value & (1 << bit) != 0 {
                    self.plot(self.x, self.band + bit);
                }
            }

            self.x += 1;
            self.max_width = self.max_width.max(self.x);
        }

        // A data character occupies the full six-pixel band even when only some
        // bits are set, so the canvas is band-aligned in height.
        self.max_height = self.max_height.max(self.band + 6);
    }

    fn plot(&mut self, x: usize, y: usize) {
        if self.rows.len() <= y {
            self.rows.resize(y + 1, Vec::new());
        }

        let row = &mut self.rows[y];

        if row.len() <= x {
            row.resize(x + 1, TRANSPARENT);
        }

        row[x] = self.color.with_alpha(255);
    }

    fn into_sixel(self) -> Option<Sixel> {
        let width = self.max_width.max(self.declared_width);
        let height = self.max_height.max(self.declared_height);

        if width == 0 || height == 0 {
            return None;
        }

        // Reject canvases whose declared size overflows or exceeds the cap,
        // rather than overflowing the multiply or attempting a huge allocation.
        let area = width.checked_mul(height).filter(|&a| a <= MAX_PIXELS)?;
        let mut pixels = vec![TRANSPARENT; area];

        for (y, row) in self.rows.iter().enumerate() {
            for (x, &px) in row.iter().enumerate() {
                pixels[y * width + x] = px;
            }
        }

        Some(Sixel {
            width,
            height,
            pixels,
        })
    }
}

/// Parse an optional decimal number at `start`, returning its value (`0` when
/// absent) and the index of the first non-digit character.
fn parse_number(chars: &[char], start: usize) -> (usize, usize) {
    let mut value = 0usize;
    let mut i = start;

    while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
        value = value.saturating_mul(10).saturating_add(d as usize);
        i += 1;
    }

    (value, i)
}

/// Parse a `;`-prefixed parameter; a missing separator or value yields `0`.
fn parse_param(chars: &[char], i: usize) -> (usize, usize) {
    match chars.get(i) {
        Some(';') => parse_number(chars, i + 1),
        _ => (0, i),
    }
}

fn rgb_from_percent(r: usize, g: usize, b: usize) -> RGB8 {
    RGB8::new(percent_to_u8(r), percent_to_u8(g), percent_to_u8(b))
}

fn percent_to_u8(v: usize) -> u8 {
    ((v.min(100) * 255 + 50) / 100) as u8
}

/// Convert DEC sixel HLS to RGB. Sixel hue is measured so that 0° is blue,
/// 120° red and 240° green — a 240° rotation of the conventional HSL wheel.
fn hls_to_rgb(h: usize, l: usize, s: usize) -> RGB8 {
    let h = ((h % 360) as f64 + 240.0) % 360.0 / 360.0;
    let l = (l.min(100) as f64) / 100.0;
    let s = (s.min(100) as f64) / 100.0;

    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return RGB8::new(v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    RGB8::new(
        (hue_to_channel(p, q, h + 1.0 / 3.0) * 255.0).round() as u8,
        (hue_to_channel(p, q, h) * 255.0).round() as u8,
        (hue_to_channel(p, q, h - 1.0 / 3.0) * 255.0).round() as u8,
    )
}

fn hue_to_channel(p: f64, q: f64, t: f64) -> f64 {
    let t = t.rem_euclid(1.0);

    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// The VT340 default 16-color palette (DEC percentages scaled to 0..=255), with
/// the remaining registers black.
fn default_palette() -> Vec<RGB8> {
    const DEFAULTS: [(usize, usize, usize); 16] = [
        (0, 0, 0),
        (20, 20, 80),
        (80, 13, 13),
        (20, 80, 20),
        (80, 20, 80),
        (20, 80, 80),
        (80, 80, 20),
        (53, 53, 53),
        (26, 26, 26),
        (33, 33, 60),
        (60, 26, 26),
        (33, 60, 33),
        (60, 33, 60),
        (33, 60, 60),
        (60, 60, 33),
        (80, 80, 80),
    ];

    let mut palette = vec![RGB8::new(0, 0, 0); 256];

    for (i, (r, g, b)) in DEFAULTS.into_iter().enumerate() {
        palette[i] = rgb_from_percent(r, g, b);
    }

    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque(r: u8, g: u8, b: u8) -> RGBA8 {
        RGBA8::new(r, g, b, 255)
    }

    #[test]
    fn decodes_single_red_pixel() {
        let s = decode("#0;2;100;0;0@").unwrap();

        assert_eq!((s.width, s.height), (1, 6));
        assert_eq!(s.pixels[0], opaque(255, 0, 0));
        assert!(s.pixels[1..].iter().all(|p| p.a == 0));
    }

    #[test]
    fn honors_raster_dimensions() {
        let s = decode("\"1;1;4;12#0;2;100;100;100@").unwrap();

        assert_eq!((s.width, s.height), (4, 12));
        assert_eq!(s.pixels.len(), 48);
    }

    #[test]
    fn run_length_and_bands() {
        let s = decode("#0;2;0;0;100!5~-!5~").unwrap();

        assert_eq!((s.width, s.height), (5, 12));
        assert!(s.pixels.iter().all(|p| *p == opaque(0, 0, 255)));
    }

    #[test]
    fn hls_primaries_match_dec_wheel() {
        assert_eq!(hls_to_rgb(0, 50, 100), RGB8::new(0, 0, 255));
        assert_eq!(hls_to_rgb(120, 50, 100), RGB8::new(255, 0, 0));
        assert_eq!(hls_to_rgb(240, 50, 100), RGB8::new(0, 255, 0));
    }

    #[test]
    fn empty_data_is_none() {
        assert!(decode("").is_none());
    }

    #[test]
    fn oversized_raster_declaration_is_rejected() {
        // A malformed raster declaration can claim an enormous canvas while
        // sending almost no pixels. Allocating width * height pixels for it
        // would overflow or OOM the process, so decode must bail instead.
        let s = decode("\"1;1;999999999;999999999#0;2;100;0;0@");
        assert!(s.is_none(), "expected oversized raster to be rejected");
    }
}
