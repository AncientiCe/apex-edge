//! Document generation: render documents and make them available to POS consumers.

pub mod generator;
pub mod pdf;
pub mod render;

pub use generator::*;
pub use pdf::*;
pub use render::*;
