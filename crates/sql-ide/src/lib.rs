//! GUI-agnostic SQL IDE support for `datafusion-ui`.
//!
//! Three independent capabilities, each a free function over `&str`:
//! - [`lex`] / [`highlight_spans`] — tokenize for syntax highlighting.
//! - [`complete`] — schema-aware autocomplete driven by a [`Catalog`].
//! - [`diagnostics`] — parser-only syntax validation.
//!
//! This crate deliberately has no GUI dependency. Positions follow sqlparser's
//! convention (1-based line and column); the caller is responsible for any
//! conversion to a 0-based editor coordinate system.

mod catalog;
mod complete;
mod diagnostics;
mod lex;

pub use catalog::{Catalog, ColumnMeta, Database, SchemaNs, TableMeta};
pub use complete::{Completion, CompletionKind, complete};
pub use diagnostics::{Diagnostic, diagnostics};
pub use lex::{SpannedToken, TokenKind, highlight_spans, lex};
