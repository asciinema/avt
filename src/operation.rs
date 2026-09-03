use crate::parser::Function;

/// One executable terminal action, observed before and after execution.
/// The parser produces [`Function`] values; resizes are requested
/// explicitly by the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    Function(Function),
    /// Changes the virtual terminal's dimensions. Dimensions are clamped to at
    /// least 1 before execution and observation. A window-resize request parsed
    /// from an escape sequence (`Function::Xtwinops`) does not change these
    /// dimensions.
    Resize {
        cols: usize,
        rows: usize,
    },
}

impl From<Function> for Operation {
    fn from(function: Function) -> Self {
        Self::Function(function)
    }
}

/// Inferred dispatch marker for a single executable action.
pub struct Single;

/// Inferred dispatch marker for a flat iterable of executable actions.
pub struct Sequence;

/// Converts an execution argument into its ordinary iterator of owned actions.
/// `Kind` distinguishes single actions from sequences and is normally inferred.
pub trait IntoOperations<Kind> {
    type Iter: Iterator<Item: Into<Operation>>;

    fn into_operations(self) -> Self::Iter;
}

impl<T: Into<Operation>> IntoOperations<Single> for T {
    type Iter = std::iter::Once<T>;

    fn into_operations(self) -> Self::Iter {
        std::iter::once(self)
    }
}

impl<I> IntoOperations<Sequence> for I
where
    I: IntoIterator,
    I::Item: Into<Operation>,
{
    type Iter = I::IntoIter;

    fn into_operations(self) -> Self::Iter {
        self.into_iter()
    }
}
