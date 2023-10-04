use std::ops::Range;

#[derive(Debug)]
pub struct DirtyLines(Vec<bool>);

impl DirtyLines {
    pub fn new(len: usize) -> Self {
        DirtyLines(vec![true; len])
    }

    pub fn add(&mut self, n: usize) {
        self.0[n] = true;
    }

    pub fn extend(&mut self, range: Range<usize>) {
        self.0[range].fill(true);
    }

    pub fn resize(&mut self, len: usize) {
        self.0.resize(len, false);
    }

    pub fn clear(&mut self) {
        self.0[..].fill(false);
    }

    pub fn to_vec(&self) -> Vec<usize> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(i, &affected)| if affected { Some(i) } else { None })
            .collect()
    }
}
