//! Single fragments and flat sequences accepted by terminal feeds.

use alloc::string::String;

/// One text fragment or character. Text is borrowed; characters are stored inline.
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Text(&'a str),
    Char(char),
}

/// Borrows a single fragment for synchronous processing.
pub trait AsInput {
    fn as_input(&self) -> Input<'_>;
}

impl AsInput for str {
    fn as_input(&self) -> Input<'_> {
        Input::Text(self)
    }
}

impl AsInput for String {
    fn as_input(&self) -> Input<'_> {
        self.as_str().as_input()
    }
}

impl AsInput for char {
    fn as_input(&self) -> Input<'_> {
        Input::from(*self)
    }
}

impl AsInput for Input<'_> {
    fn as_input(&self) -> Input<'_> {
        *self
    }
}

impl<T: AsInput + ?Sized> AsInput for &T {
    fn as_input(&self) -> Input<'_> {
        (**self).as_input()
    }
}

impl<T: AsInput + ?Sized> AsInput for &mut T {
    fn as_input(&self) -> Input<'_> {
        (**self).as_input()
    }
}

impl<'a, T: AsInput + ?Sized> From<&'a T> for Input<'a> {
    fn from(value: &'a T) -> Self {
        value.as_input()
    }
}

impl From<char> for Input<'_> {
    fn from(value: char) -> Self {
        Self::Char(value)
    }
}

/// Inferred dispatch marker for a single fragment.
pub struct Single;

/// Inferred dispatch marker for a flat sequence of fragments.
pub struct Sequence;

/// Converts a feed argument into its ordinary iterator of fragments.
/// The terminal borrows each item while processing it, including owned strings.
/// `Kind` distinguishes single fragments from sequences and is normally inferred.
pub trait IntoInputs<Kind> {
    type Iter: Iterator<Item: AsInput>;

    fn into_inputs(self) -> Self::Iter;
}

impl<T: AsInput> IntoInputs<Single> for T {
    type Iter = core::iter::Once<T>;

    fn into_inputs(self) -> Self::Iter {
        core::iter::once(self)
    }
}

impl<I> IntoInputs<Sequence> for I
where
    I: IntoIterator,
    I::Item: AsInput,
{
    type Iter = I::IntoIter;

    fn into_inputs(self) -> Self::Iter {
        self.into_iter()
    }
}
