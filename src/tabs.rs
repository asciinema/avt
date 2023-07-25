#[derive(Debug, Clone)]
pub(crate) struct Tabs(Vec<usize>);

impl Tabs {
    pub fn new(cols: usize) -> Self {
        let mut tabs = vec![];

        for t in (8..cols).step_by(8) {
            tabs.push(t);
        }

        Tabs(tabs)
    }

    pub fn set(&mut self, pos: usize) {
        if let Err(index) = self.0.binary_search(&pos) {
            self.0.insert(index, pos);
        }
    }

    pub fn unset(&mut self, pos: usize) {
        if let Ok(index) = self.0.binary_search(&pos) {
            self.0.remove(index);
        }
    }

    pub fn expand(&mut self, mut start: usize, end: usize) {
        start += 8 - start % 8;

        for t in (start..end).step_by(8) {
            self.0.push(t);
        }
    }

    pub fn contract(&mut self, pos: usize) {
        let index = self.0.partition_point(|t| t < &pos);
        self.0.truncate(index);
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn before(&self, pos: usize, n: usize) -> Option<usize> {
        self.0
            .iter()
            .rev()
            .skip_while(|t| pos <= **t)
            .nth(n - 1)
            .copied()
    }

    pub fn after(&self, pos: usize, n: usize) -> Option<usize> {
        self.0.iter().skip_while(|t| pos >= **t).nth(n - 1).copied()
    }
}

impl<'a> IntoIterator for &'a Tabs {
    type Item = &'a usize;
    type IntoIter = std::slice::Iter<'a, usize>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl PartialEq for Tabs {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<Vec<usize>> for Tabs {
    fn eq(&self, other: &Vec<usize>) -> bool {
        &self.0 == other
    }
}

#[cfg(test)]
mod tests {
    use super::Tabs;

    #[test]
    fn new() {
        assert_eq!(Tabs::new(1), vec![]);
        assert_eq!(Tabs::new(8), vec![]);
        assert_eq!(Tabs::new(9), vec![8]);
        assert_eq!(Tabs::new(16), vec![8]);
        assert_eq!(Tabs::new(17), vec![8, 16]);
    }
}
