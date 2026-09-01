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
