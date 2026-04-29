use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("undefined variable: {0}")]
    UndefinedVar(String),

    #[error("unknown unit: {0}")]
    UnknownUnit(String),

    #[error("offset unit `{0}` not supported — poppop only handles linear unit conversions, so absolute temperatures (celsius/fahrenheit/rankine) silently give wrong answers. Use `kelvin` for absolute temperatures, or express temperature differences in `kelvin` (1 K = 1 °C interval)")]
    OffsetUnit(String),

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
