use crate::ast::UnitExpr;
use crate::unit::Unit;

#[derive(Clone, Debug)]
pub struct Value {
    pub mag: f64,
    pub unit: Unit,
    /// Preferred display unit, propagated through arithmetic so that
    /// `42/4 mph` renders as `10.5 mph` rather than `4.69 m/s`. The
    /// formatter only honors this if it resolves to the same base unit.
    pub display: Option<UnitExpr>,
}

impl Value {
    pub fn new(mag: f64, unit: Unit) -> Self {
        Value {
            mag,
            unit,
            display: None,
        }
    }

    pub fn with_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        Value {
            mag,
            unit,
            display: Some(display),
        }
    }

    pub fn dimensionless(mag: f64) -> Self {
        Value::new(mag, Unit::DIMENSIONLESS)
    }
}
