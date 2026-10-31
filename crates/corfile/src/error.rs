//! Errors for parsing and building correction tables.

use thiserror::Error;

/// Why a correction file or node list was rejected.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CorError {
    /// The file ends before the grid does.
    #[error("correction file too short: need at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    /// Neither `JCZ_COR_2_1` nor `LMC1COR_1.0`. `label` is a printable rendering of the first bytes.
    #[error("unknown correction file format (label {label:?}); expected JCZ_COR_2_1 or LMC1COR_1.0")]
    UnknownFormat { label: String },
    /// A grid built in code has the wrong number of nodes.
    #[error("correction grid must have {expected} nodes, got {actual}")]
    NodeCount { expected: usize, actual: usize },
    /// An offset is not finite or does not fit the 15-bit magnitude of `WriteCorLine`.
    #[error("correction offset {value} at row {row}, col {col} is outside ±32767 galvo units")]
    OffsetOutOfRange { row: usize, col: usize, value: f64 },
}
