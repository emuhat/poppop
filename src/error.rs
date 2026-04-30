use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("undefined variable: {0}")]
    UndefinedVar(String),

    #[error("unknown unit: {0}")]
    UnknownUnit(String),

    #[error("temperature arithmetic: {0}")]
    TempArithmetic(String),

    #[error("unknown function: {0}")]
    UnknownFn(String),

    #[error("{name} expects {expected} argument(s), got {got}")]
    Arity {
        name: String,
        expected: usize,
        got: usize,
    },

    #[error("unit mismatch: {left} vs {right}")]
    UnitMismatch { left: String, right: String },

    #[error("dimension error: {0}")]
    DimError(String),
}
