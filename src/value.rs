use chrono::NaiveDateTime;

use crate::ast::UnitExpr;
use crate::unit::Unit;

/// Top-level value type. Three flavors:
///
/// * `Linear` — scalar quantity with units. Covers durations, masses,
///   speeds, temperatures, dimensionless numbers, everything pre-dating
///   the date/time work. Carries the temperature flags from the affine
///   temperature spec.
/// * `Instant` — absolute point in time (timestamp).
/// * `Period` — calendar period stored as `(years, months, days)`. Not
///   reducible to a duration without an anchor.
///
/// Per the time spec: arithmetic decisions branch on this variant
/// (`is_linear()`, `is_instant()`, `is_period()`) ONLY. Never on the
/// stored `unit`, atom names, or magnitude.
#[derive(Clone, Debug)]
pub enum Value {
    Linear(LinearValue),
    Instant(InstantValue),
    Period(PeriodValue),
}

/// All current scalar-with-unit behavior lives here. Field shape matches
/// the previous `Value` struct exactly so existing eval/format code
/// migrates to `match v { Value::Linear(l) => …, _ => … }` with minimal
/// churn.
#[derive(Clone, Debug, Default)]
pub struct LinearValue {
    pub mag: f64,
    pub unit: Unit,
    pub display: Option<UnitExpr>,
    pub display_explicit: bool,
    pub force_hms: bool,
    pub is_absolute_temp: bool,
    pub render_as_delta: bool,
}

#[derive(Clone, Debug)]
pub struct InstantValue {
    /// Timestamp in UTC. Stored as NaiveDateTime; we add timezone
    /// support in a later iteration.
    pub timestamp: NaiveDateTime,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeriodValue {
    pub years: i32,
    pub months: i32,
    pub days: i32,
}

impl LinearValue {
    pub fn new(mag: f64, unit: Unit) -> Self {
        LinearValue { mag, unit, ..Default::default() }
    }

    pub fn with_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        LinearValue { mag, unit, display: Some(display), ..Default::default() }
    }

    pub fn with_explicit_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        LinearValue {
            mag,
            unit,
            display: Some(display),
            display_explicit: true,
            ..Default::default()
        }
    }

    pub fn with_force_hms(mag: f64, unit: Unit) -> Self {
        LinearValue { mag, unit, force_hms: true, ..Default::default() }
    }

    pub fn absolute_temp(mag_in_kelvin: f64, display: UnitExpr) -> Self {
        LinearValue {
            mag: mag_in_kelvin,
            unit: Unit::KELVIN,
            display: Some(display),
            is_absolute_temp: true,
            ..Default::default()
        }
    }

    pub fn dimensionless(mag: f64) -> Self {
        LinearValue::new(mag, Unit::DIMENSIONLESS)
    }
}

impl Value {
    /// Compatibility constructor. Most existing call sites build linear
    /// scalar values; they keep working unchanged through this.
    pub fn new(mag: f64, unit: Unit) -> Self {
        Value::Linear(LinearValue::new(mag, unit))
    }

    pub fn with_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        Value::Linear(LinearValue::with_display(mag, unit, display))
    }

    pub fn with_explicit_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        Value::Linear(LinearValue::with_explicit_display(mag, unit, display))
    }

    pub fn with_force_hms(mag: f64, unit: Unit) -> Self {
        Value::Linear(LinearValue::with_force_hms(mag, unit))
    }

    pub fn absolute_temp(mag_in_kelvin: f64, display: UnitExpr) -> Self {
        Value::Linear(LinearValue::absolute_temp(mag_in_kelvin, display))
    }

    pub fn dimensionless(mag: f64) -> Self {
        Value::Linear(LinearValue::dimensionless(mag))
    }

    pub fn instant(timestamp: NaiveDateTime) -> Self {
        Value::Instant(InstantValue { timestamp })
    }

    pub fn period(years: i32, months: i32, days: i32) -> Self {
        Value::Period(PeriodValue { years, months, days })
    }

    pub fn as_linear(&self) -> Option<&LinearValue> {
        match self {
            Value::Linear(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_instant(&self) -> Option<&InstantValue> {
        match self {
            Value::Instant(i) => Some(i),
            _ => None,
        }
    }

    pub fn as_period(&self) -> Option<&PeriodValue> {
        match self {
            Value::Period(p) => Some(p),
            _ => None,
        }
    }

    pub fn is_linear(&self) -> bool {
        matches!(self, Value::Linear(_))
    }

    pub fn is_instant(&self) -> bool {
        matches!(self, Value::Instant(_))
    }

    pub fn is_period(&self) -> bool {
        matches!(self, Value::Period(_))
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::Linear(LinearValue::default())
    }
}
