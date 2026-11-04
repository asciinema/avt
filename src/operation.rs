use crate::ansi::Function;

/// One executable terminal action, observed before and after execution.
/// The parser produces [`Function`] values; resizes are requested
/// explicitly by the caller. Print text may be borrowed; use
/// [`Operation::into_owned`] to retain an action independently of its input.
#[derive(Debug, Clone, PartialEq)]
pub enum Operation<'a> {
    Function(Function<'a>),
    /// Changes the virtual terminal's dimensions. Dimensions are clamped to at
    /// least 1 before execution and observation. A window-resize request parsed
    /// from an escape sequence (`Function::Xtwinops`) does not change these
    /// dimensions.
    Resize {
        cols: usize,
        rows: usize,
    },
}

impl Operation<'_> {
    /// Copies borrowed print text and moves already-owned data, making the
    /// operation independent of the input fragment that produced it.
    pub fn into_owned(self) -> Operation<'static> {
        match self {
            Self::Function(function) => Operation::Function(function.into_owned()),
            Self::Resize { cols, rows } => Operation::Resize { cols, rows },
        }
    }
}

impl<'a> From<Function<'a>> for Operation<'a> {
    fn from(function: Function<'a>) -> Self {
        Self::Function(function)
    }
}

/// Inferred dispatch marker for a single executable action.
pub struct Single;

/// Inferred dispatch marker for a flat iterable of executable actions.
pub struct Sequence;

/// Converts an execution argument into its ordinary iterator of actions.
/// Actions are moved into execution, but their print text may borrow input.
/// `Kind` distinguishes single actions from sequences and is normally inferred.
pub trait IntoOperations<'a, Kind> {
    type Iter: Iterator<Item: Into<Operation<'a>>>;

    fn into_operations(self) -> Self::Iter;
}

impl<'a, T: Into<Operation<'a>>> IntoOperations<'a, Single> for T {
    type Iter = core::iter::Once<T>;

    fn into_operations(self) -> Self::Iter {
        core::iter::once(self)
    }
}

impl<'a, I> IntoOperations<'a, Sequence> for I
where
    I: IntoIterator,
    I::Item: Into<Operation<'a>>,
{
    type Iter = I::IntoIter;

    fn into_operations(self) -> Self::Iter {
        self.into_iter()
    }
}
