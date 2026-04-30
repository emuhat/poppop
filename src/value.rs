use crate::ast::UnitExpr;
use crate::unit::Unit;

#[derive(Clone, Debug, Default)]
pub struct Value {
    pub mag: f64,
    pub unit: Unit,
    /// Preferred display unit, propagated through arithmetic so that
    /// `42/4 mph` renders as `10.5 mph` rather than `4.69 m/s`. The
    /// formatter only honors this if it resolves to the same base unit.
    pub display: Option<UnitExpr>,
    /// True when the display hint came from an explicit `in <unit>`
    /// conversion. Lets the formatter override HMS auto-detect:
    /// `14 hours in seconds` should print `50400 seconds`, not
    /// `14:00:00.00`. Arithmetic clears the flag.
    pub display_explicit: bool,
    /// True when the user wrote `in hms` to force HMS rendering on a
    /// pure-time value (e.g. `42 hours in hms` → `42:00:00.00`). Mutually
    /// exclusive with `display_explicit` in practice. Arithmetic clears it.
    pub force_hms: bool,
    /// **Pre-normalization origin marker for affine temperatures.**
    /// True when this value's source expression named an affine
    /// temperature unit (literal `25 degC`, or any `Convert` whose
    /// source had this flag set). Travels through every conversion —
    /// `25 degC in K` keeps `is_absolute_temp = true` even though the
    /// resulting Value has `unit = K`. The flag is the ONLY thing that
    /// distinguishes `25 degC in K` from a plain `5 K` arithmetically.
    /// Cleared exactly once: by absolute−absolute subtraction (which
    /// turns the result into a delta).
    ///
    /// All temperature arithmetic decisions branch on this flag. Code
    /// must NOT branch on `unit == K` to infer temperature semantics.
    pub is_absolute_temp: bool,
    /// Pure formatting hint. Set by absolute−absolute subtraction OR
    /// by converting a non-absolute into an affine display unit.
    /// Causes the formatter to render with a `Δ` prefix
    /// (e.g. `Δ5 °C`). Has no effect on arithmetic, conversion, or
    /// any other semantic decision. Preserved through conversions.
    pub render_as_delta: bool,
}

impl Value {
    pub fn new(mag: f64, unit: Unit) -> Self {
        Value { mag, unit, ..Default::default() }
    }

    pub fn with_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        Value { mag, unit, display: Some(display), ..Default::default() }
    }

    pub fn with_explicit_display(mag: f64, unit: Unit, display: UnitExpr) -> Self {
        Value {
            mag,
            unit,
            display: Some(display),
            display_explicit: true,
            ..Default::default()
        }
    }

    pub fn with_force_hms(mag: f64, unit: Unit) -> Self {
        Value { mag, unit, force_hms: true, ..Default::default() }
    }

    /// Constructor for an affine temperature literal. The magnitude is
    /// already in Kelvin (offset-applied at the call site).
    pub fn absolute_temp(mag_in_kelvin: f64, display: UnitExpr) -> Self {
        Value {
            mag: mag_in_kelvin,
            unit: Unit::KELVIN,
            display: Some(display),
            is_absolute_temp: true,
            ..Default::default()
        }
    }

    pub fn dimensionless(mag: f64) -> Self {
        Value::new(mag, Unit::DIMENSIONLESS)
    }
}
