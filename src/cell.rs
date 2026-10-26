use crate::pen::Pen;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Bytes reserved for one grapheme cluster. Family ZWJ sequences fit in 25.
pub(crate) const MAX_CLUSTER_BYTES: usize = 32;

/// The number of terminal columns occupied by a single grapheme cluster.
pub(crate) fn display_width(cluster: &str) -> usize {
    cluster.width().clamp(1, 2)
}

pub(crate) fn clusters_form_one_grapheme(left: &str, right: &str) -> bool {
    let mut combined = String::with_capacity(left.len() + right.len());
    combined.push_str(left);
    combined.push_str(right);
    let mut graphemes = combined.graphemes(true);
    graphemes.next().is_some() && graphemes.next().is_none()
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct Cluster {
    bytes: [u8; MAX_CLUSTER_BYTES],
    len: u8,
}

impl Cluster {
    const fn space() -> Self {
        let mut bytes = [0u8; MAX_CLUSTER_BYTES];
        bytes[0] = b' ';
        Self { bytes, len: 1 }
    }

    fn from_str(s: &str) -> Self {
        let mut bytes = [0u8; MAX_CLUSTER_BYTES];
        let mut len = 0usize;

        for ch in s.chars() {
            let n = ch.len_utf8();
            if len + n > MAX_CLUSTER_BYTES {
                debug_assert!(
                    false,
                    "grapheme exceeds {MAX_CLUSTER_BYTES} inline bytes: {s:?}"
                );
                break;
            }
            ch.encode_utf8(&mut bytes[len..]);
            len += n;
        }

        if len == 0 {
            return Self::space();
        }

        Self {
            bytes,
            len: len as u8,
        }
    }

    fn as_str(&self) -> &str {
        // Only UTF-8 scalars are written in `from_str`.
        unsafe { std::str::from_utf8_unchecked(&self.bytes[..self.len as usize]) }
    }
}

impl std::fmt::Debug for Cluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell(Cluster, Occupancy, Pen);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Occupancy {
    Single,
    WideHead,
    WideTail,
}

impl Occupancy {
    pub(crate) fn width(&self) -> u8 {
        match self {
            Occupancy::Single => 1,
            Occupancy::WideHead => 2,
            Occupancy::WideTail => 0,
        }
    }
}

impl Cell {
    pub(crate) fn new(contents: &str, occupancy: Occupancy, pen: Pen) -> Self {
        Cell(Cluster::from_str(contents), occupancy, pen)
    }

    pub(crate) fn blank(pen: Pen) -> Self {
        Cell(Cluster::space(), Occupancy::Single, pen)
    }

    pub fn is_default(&self) -> bool {
        self.0.as_str() == " " && self.1 == Occupancy::Single && self.2.is_default()
    }

    /// Returns the complete grapheme cluster stored in this cell.
    pub fn contents(&self) -> &str {
        self.0.as_str()
    }

    /// Returns the first scalar value of the cell's grapheme cluster.
    ///
    /// Prefer [`Cell::contents`] when rendering text, as a cell can contain
    /// multiple Unicode scalar values.
    pub fn char(&self) -> char {
        self.0.as_str().chars().next().unwrap_or(' ')
    }

    pub(crate) fn occupancy(&self) -> Occupancy {
        self.1
    }

    pub fn width(&self) -> u8 {
        self.1.width()
    }

    pub fn pen(&self) -> &Pen {
        &self.2
    }

    pub(crate) fn set(&mut self, contents: &str, occupancy: Occupancy, pen: Pen) {
        self.0 = Cluster::from_str(contents);
        self.1 = occupancy;
        self.2 = pen;
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Pen::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_storage_does_not_need_drop() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Cell>();
        assert!(
            !std::mem::needs_drop::<Cell>(),
            "Cell holds a heap String ({} bytes); cluster text should be inline so Cell can stay Copy",
            std::mem::size_of::<Cell>()
        );
    }

    #[test]
    fn family_zwj_emoji_fits_inline() {
        let cluster = "👨‍👩‍👧‍👦";
        assert!(cluster.len() <= MAX_CLUSTER_BYTES);
        let cell = Cell::new(cluster, Occupancy::WideHead, Pen::default());
        assert_eq!(cell.contents(), cluster);
    }
}
