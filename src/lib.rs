mod cell;
mod charset;
mod color;
mod line;
mod pen;
mod saved_ctx;
mod segment;
mod vt;
use cell::Cell;
use charset::Charset;
pub use color::Color;
pub use line::Line;
use pen::Intensity;
pub use pen::Pen;
use saved_ctx::SavedCtx;
pub use segment::Segment;
pub use vt::Vt;

trait Dump {
    fn dump(&self) -> String;
}

#[cfg(test)]
#[macro_use(quickcheck)]
extern crate quickcheck_macros;
