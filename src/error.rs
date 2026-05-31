//! Typed errors for the app, replacing the old `Result<T, String>` convention.
//!
//! Errors flow through Iced's `Message` enum, and Iced requires `Message: Clone`.
//! None of the underlying source errors (`DataFusionError`, `std::io::Error`,
//! `tonic::Status`, `ArrowError`, …) are `Clone`, so every variant here carries a
//! **rendered `String`** of the source rather than the source itself. That keeps
//! the whole tree `Clone` while still giving callers a matchable, typed error.
//!
//! Layering: per-domain enums ([`ParquetError`], [`QueryError`], [`FlightError`],
//! [`ExportError`]) compose into the top-level [`AppError`] via
//! `#[from]`. Functions that span backends (e.g. the `QueryEngine`, which runs on
//! either a local DataFusion session or a FlightSQL connection) return `AppError`
//! directly; single-domain functions return their domain error.

/// The aggregate application error. Domain errors convert into it with `?`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Parquet(#[from] ParquetError),
    #[error(transparent)]
    Query(#[from] QueryError),
    #[error(transparent)]
    Flight(#[from] FlightError),
    #[error(transparent)]
    Export(#[from] ExportError),
}

/// Reading Parquet file metadata (`parquet_io`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParquetError {
    #[error("join error: {0}")]
    Join(String),
    #[error("stat failed: {0}")]
    Stat(String),
    #[error("open failed: {0}")]
    Open(String),
    #[error("not a valid parquet file: {0}")]
    InvalidParquet(String),
}

/// Running SQL against a local DataFusion session (`engine`, `wrangle::session`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum QueryError {
    /// Planning/parsing a SQL statement failed.
    #[error("sql: {0}")]
    Plan(String),
    /// Executing or collecting query results failed.
    #[error("execute: {0}")]
    Collect(String),
    /// Registering / looking up a table in the session failed.
    #[error("register table: {0}")]
    Register(String),
    /// Concatenating record batches failed.
    #[error("concat: {0}")]
    Concat(String),
}

/// Talking to a FlightSQL server (`flightsql`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum FlightError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("connect: {0}")]
    Connect(String),
    /// A FlightSQL RPC failed; `op` names the call (e.g. `"execute"`, `"do_get"`).
    #[error("{op}: {msg}")]
    Rpc { op: &'static str, msg: String },
    /// A metadata result set was missing a column or had an unexpected type.
    #[error("metadata: {0}")]
    Metadata(String),
}

/// Writing a query result to disk (`export`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExportError {
    #[error("create file: {0}")]
    CreateFile(String),
    /// A write step failed; `op` names it (e.g. `"write parquet"`, `"read batch"`).
    #[error("{op}: {msg}")]
    Write { op: &'static str, msg: String },
}
