use std::collections::HashMap;

use crate::ast::*;
use crate::builtins;
use crate::error::Error;
use crate::unit::{Resolved, Unit, UnitRegistry};
use crate::value::Value;

pub struct Env {
    /// Keys stored lowercased — variable names are case-insensitive.
    /// `X = 5` and `x = 10` both refer to the same binding. Unit lookups
    /// stay case-sensitive (K vs k matters: kelvin vs kilo).
    vars: HashMap<String, Value>,
    pub registry: UnitRegistry,
}

impl Env {
    pub fn new() -> Self {
        let mut env = Env {
            vars: HashMap::new(),
            registry: UnitRegistry::standard(),
        };
        // TI-82 style: Ans starts at 0 so referencing it before any
        // computation gives a sensible value rather than an error.
        env.set("Ans", Value::dimensionless(0.0));
        env
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.vars.get(&name.to_ascii_lowercase())
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.vars.insert(name.to_ascii_lowercase(), value);
    }

    /// Drop user bindings and re-bind `Ans = 0`. The unit registry is left
    /// intact — this is the cheap path for "recompute the whole notebook"
    /// frontends that want a fresh env on every render but can't afford the
    /// pint loader's multi-pass cost.
    pub fn reset(&mut self) {
        self.vars.clear();
        self.set("Ans", Value::dimensionless(0.0));
    }
}

#[derive(Debug, Clone)]
pub enum Answer {
    Bare(Value),
    Assigned { name: String, value: Value },
}

pub struct Engine {
    env: Env,
}

impl Engine {
    pub fn new() -> Self {
        Engine { env: Env::new() }
    }

    /// Cheap state reset: clears user bindings and resets `Ans`, but keeps
    /// the (expensive) unit registry. Use this when re-evaluating from
    /// scratch on every edit (e.g. notebook frontends).
    pub fn reset(&mut self) {
        self.env.reset();
    }

    pub fn eval(&mut self, input: &str) -> Result<Answer, Error> {
        let stmt = crate::parser::parse(input)?;
        let answer = match stmt {
            Statement::Assign(name, rhs) => {
                let value = eval_expr(&mut self.env, &rhs)?;
                self.env.set(&name, value.clone());
                Answer::Assigned { name, value }
            }
            Statement::Expr(e) => Answer::Bare(eval_expr(&mut self.env, &e)?),
        };
        // TI-82 style: Ans tracks the most recent result. Failed evals
        // don't update Ans (we only get here on Ok).
        let last_value = match &answer {
            Answer::Bare(v) => v.clone(),
            Answer::Assigned { value, .. } => value.clone(),
        };
        self.env.set("Ans", last_value);
        Ok(answer)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

pub fn eval_expr(env: &mut Env, e: &Expr) -> Result<Value, Error> {
    match e {
        Expr::Number(n, None) => Ok(Value::dimensionless(*n)),
        Expr::Number(n, Some(u)) => {
            let r = env.registry.resolve(u)?;
            // Affine temperature literal: single-atom resolution carries
            // affine metadata; apply offset and mark the result absolute.
            // `is_absolute_temp` is set ONLY here at literal-construction
            // time (and propagated through Convert). Eval never infers it
            // from `unit` or `display` later.
            if let Some(atom) = r.atom {
                if atom.is_affine_temp {
                    let mag = n * atom.factor + atom.offset;
                    return Ok(Value::absolute_temp(mag, u.clone()));
                }
            }
            Ok(Value::with_display(n * r.factor, r.unit, u.clone()))
        }
        Expr::Var(name) => {
            // User-bound variables shadow units. Falling back to the registry lets
            // bare unit names like `mph` evaluate to `0.44704 m/s`, so implicit
            // multiplication (`4 mph` → `4 * mph`) gives the natural result.
            if let Some(v) = env.get(name) {
                return Ok(v.clone());
            }
            let atom = UnitExpr::Atom(name.clone(), 1);
            match env.registry.resolve(&atom) {
                Ok(r) => {
                    // A bare affine atom name (e.g. `degC` alone) resolves to
                    // "1 in that unit," which for Celsius is 1 °C = 274.15 K
                    // absolute.
                    if let Some(def) = r.atom {
                        if def.is_affine_temp {
                            let mag = 1.0 * def.factor + def.offset;
                            return Ok(Value::absolute_temp(mag, atom));
                        }
                    }
                    Ok(Value::with_display(r.factor, r.unit, atom))
                }
                Err(_) => Err(Error::UndefinedVar(name.clone())),
            }
        }
        Expr::Unary(UnaryOp::Neg, inner) => {
            let v = eval_expr(env, inner)?;
            // Negation preserves all flags. `-(25 degC)` is conceptually a
            // negative absolute reading; the user asked for it explicitly.
            Ok(Value {
                mag: -v.mag,
                unit: v.unit,
                display: v.display,
                display_explicit: v.display_explicit,
                force_hms: v.force_hms,
                is_absolute_temp: v.is_absolute_temp,
                render_as_delta: v.render_as_delta,
            })
        }
        Expr::Binary(a, op, b) => {
            // Affine-literal shortcut: `25 degC`, `100 degF`, etc. parse as
            // `Number(n) * Var(ident)` because the grammar uses units-as-
            // values + implicit multiplication. If `ident` is a registered
            // affine atom, treat the whole `<number> * <affine>` as the
            // literal `n <affine>` and apply the offset (so the result is
            // an absolute temperature with `is_absolute_temp = true`).
            //
            // Without this shortcut, evaluation would naively multiply 25
            // by `1 degC` (which itself is 274.15 K absolute), and that
            // multiplication would correctly error per the spec — but
            // `25 degC` is the canonical literal form, not a scalar
            // multiplication. Recognizing the AST shape is the simplest
            // way to bridge the grammar to the spec.
            if let Op::Mul = op {
                if let Some(v) = try_affine_literal(env, a, b) {
                    return Ok(v);
                }
            }
            let va = eval_expr(env, a)?;
            let vb = eval_expr(env, b)?;
            match op {
                Op::Add => add_values(va, vb),
                Op::Sub => sub_values(va, vb),
                Op::Mul => mul_values(va, vb),
                Op::Div => div_values(va, vb),
                Op::Pow => pow_values(va, vb),
            }
        }
        Expr::Call(name, args) => {
            let mut vs = Vec::with_capacity(args.len());
            for a in args {
                vs.push(eval_expr(env, a)?);
            }
            builtins::call(name, vs)
        }
        Expr::Convert(inner, target) => convert_value(env, inner, target),
    }
}

/// Detect `Number(n) * Var(name)` where `name` resolves to an affine
/// atom. This is how the grammar represents `25 degC` (because we use
/// units-as-values with implicit multiplication). Returns the absolute
/// temperature literal in Kelvin.
fn try_affine_literal(env: &Env, a: &Expr, b: &Expr) -> Option<Value> {
    let (n, name) = match (a, b) {
        (Expr::Number(n, None), Expr::Var(name)) => (*n, name),
        (Expr::Var(name), Expr::Number(n, None)) => (*n, name),
        _ => return None,
    };
    // User-bound variables shadow units; if `name` is bound, it's not an
    // affine literal pattern.
    if env.get(name).is_some() {
        return None;
    }
    let r = env
        .registry
        .resolve(&UnitExpr::Atom(name.clone(), 1))
        .ok()?;
    let atom = r.atom?;
    if !atom.is_affine_temp {
        return None;
    }
    let mag = n * atom.factor + atom.offset;
    Some(Value::absolute_temp(mag, UnitExpr::Atom(name.clone(), 1)))
}

// ─── arithmetic ────────────────────────────────────────────────────────────
//
// Branch ONLY on `is_absolute_temp`. NEVER on `unit`, `display`, or atom
// metadata. K is storage-only — the flag carries the semantic category.

fn add_values(va: Value, vb: Value) -> Result<Value, Error> {
    match (va.is_absolute_temp, vb.is_absolute_temp) {
        (true, true) => Err(Error::TempArithmetic(
            "cannot add two absolute temperatures".to_string(),
        )),
        (true, false) | (false, true) => {
            // Absolute + linear → absolute. Magnitude is straight K addition;
            // the absolute-flagged operand's display propagates so the result
            // renders in the user's chosen scale.
            require_same_unit(&va, &vb)?;
            let (abs, _other) = if va.is_absolute_temp { (&va, &vb) } else { (&vb, &va) };
            Ok(Value {
                mag: va.mag + vb.mag,
                unit: va.unit,
                display: abs.display.clone(),
                is_absolute_temp: true,
                ..Default::default()
            })
        }
        (false, false) => {
            require_same_unit(&va, &vb)?;
            Ok(Value {
                mag: va.mag + vb.mag,
                unit: va.unit,
                display: va.display.or(vb.display),
                ..Default::default()
            })
        }
    }
}

fn sub_values(va: Value, vb: Value) -> Result<Value, Error> {
    match (va.is_absolute_temp, vb.is_absolute_temp) {
        (true, true) => {
            // Absolute - absolute → delta in K. The single case that clears
            // is_absolute_temp and sets render_as_delta.
            require_same_unit(&va, &vb)?;
            Ok(Value {
                mag: va.mag - vb.mag,
                unit: va.unit,
                display: None,
                render_as_delta: true,
                ..Default::default()
            })
        }
        (true, false) => {
            // Absolute - linear → absolute. (e.g. `25 degC - 5 K = 20 °C`)
            require_same_unit(&va, &vb)?;
            Ok(Value {
                mag: va.mag - vb.mag,
                unit: va.unit,
                display: va.display,
                is_absolute_temp: true,
                ..Default::default()
            })
        }
        (false, true) => Err(Error::TempArithmetic(
            "cannot subtract an absolute temperature from a delta".to_string(),
        )),
        (false, false) => {
            require_same_unit(&va, &vb)?;
            Ok(Value {
                mag: va.mag - vb.mag,
                unit: va.unit,
                display: va.display.or(vb.display),
                ..Default::default()
            })
        }
    }
}

fn mul_values(va: Value, vb: Value) -> Result<Value, Error> {
    if va.is_absolute_temp || vb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    Ok(Value {
        mag: va.mag * vb.mag,
        unit: va.unit.mul(&vb.unit),
        display: combine_display(va.display, vb.display, true),
        ..Default::default()
    })
}

fn div_values(va: Value, vb: Value) -> Result<Value, Error> {
    if va.is_absolute_temp || vb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    Ok(Value {
        mag: va.mag / vb.mag,
        unit: va.unit.div(&vb.unit),
        display: combine_display(va.display, vb.display, false),
        ..Default::default()
    })
}

fn pow_values(va: Value, vb: Value) -> Result<Value, Error> {
    if va.is_absolute_temp || vb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    if !vb.unit.is_dimensionless() {
        return Err(Error::DimError(
            "exponent must be dimensionless".to_string(),
        ));
    }
    let n = vb.mag;
    let int_exp = n.round() as i32;
    let is_int = (n - int_exp as f64).abs() < 1e-9;
    let unit = if is_int {
        va.unit.pow(int_exp)
    } else if va.unit.is_dimensionless() {
        va.unit
    } else {
        return Err(Error::DimError(
            "non-integer power requires a dimensionless base".to_string(),
        ));
    };
    Ok(Value::new(va.mag.powf(n), unit))
}

// ─── conversion ────────────────────────────────────────────────────────────

fn convert_value(env: &mut Env, inner: &Expr, target: &UnitExpr) -> Result<Value, Error> {
    let v = eval_expr(env, inner)?;
    // `in hms` is a display directive, not a unit conversion.
    if matches!(target, UnitExpr::Atom(name, 1) if name == "hms") {
        if !v.unit.is_pure_time() {
            return Err(Error::DimError(format!(
                "`in hms` requires a time value, got {}",
                render_unit(&v.unit)
            )));
        }
        return Ok(Value::with_force_hms(v.mag, v.unit));
    }
    let r = env.registry.resolve(target)?;
    if v.unit != r.unit {
        return Err(Error::UnitMismatch {
            left: render_unit(&v.unit),
            right: render_unit(&r.unit),
        });
    }
    // Check whether the target is a single-atom affine unit (degC/degF).
    // The decision below depends ONLY on (source.is_absolute_temp,
    // target_is_affine). Source `unit` and `display` are never inspected.
    let target_is_affine = r
        .atom
        .as_ref()
        .map(|a| a.is_affine_temp)
        .unwrap_or(false);

    if target_is_affine {
        // `<abs> in <affine>` keeps it absolute, just changes display.
        // `<linear> in <affine>` produces a delta in that scale.
        Ok(Value {
            mag: v.mag,
            unit: v.unit,
            display: Some(target.clone()),
            display_explicit: true,
            force_hms: false,
            is_absolute_temp: v.is_absolute_temp,
            render_as_delta: !v.is_absolute_temp || v.render_as_delta,
        })
    } else {
        // Target is K, a compound, or any non-affine unit.
        // Critical: is_absolute_temp propagates UNCHANGED. An `in K` on an
        // absolute keeps the absolute flag — that's how we prevent the
        // back-door `(25 degC in K) + (25 degC in K)` bug. NEVER infer
        // semantic category from `target == K`.
        Ok(Value {
            mag: v.mag,
            unit: v.unit,
            display: Some(target.clone()),
            display_explicit: true,
            force_hms: false,
            is_absolute_temp: v.is_absolute_temp,
            render_as_delta: v.render_as_delta,
        })
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn _resolved_signature(_: &Resolved) {} // keep `Resolved` reference compiling if unused

/// Combine display hints across multiplication/division. Returns `Some(combined)`
/// only when at least one side has a display; otherwise None. The combined hint
/// is the literal Mul/Div of the sides — the formatter validates dim-match before
/// using it, so junk combinations silently fall back to the base-unit renderer.
fn combine_display(
    a: Option<UnitExpr>,
    b: Option<UnitExpr>,
    is_mul: bool,
) -> Option<UnitExpr> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) => Some(x),
        (None, Some(y)) => {
            if is_mul {
                Some(y)
            } else {
                None
            }
        }
        (Some(x), Some(y)) => {
            if is_mul {
                Some(UnitExpr::Mul(Box::new(x), Box::new(y)))
            } else {
                Some(UnitExpr::Div(Box::new(x), Box::new(y)))
            }
        }
    }
}

fn require_same_unit(a: &Value, b: &Value) -> Result<(), Error> {
    if a.unit != b.unit {
        Err(Error::UnitMismatch {
            left: render_unit(&a.unit),
            right: render_unit(&b.unit),
        })
    } else {
        Ok(())
    }
}

fn render_unit(u: &Unit) -> String {
    if u.is_dimensionless() {
        "dimensionless".to_string()
    } else {
        u.render()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_str(s: &str) -> Value {
        let mut eng = Engine::new();
        match eng.eval(s).unwrap() {
            Answer::Bare(v) => v,
            Answer::Assigned { value, .. } => value,
        }
    }

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn pure_number() {
        let v = eval_str("42");
        assert_eq!(v.mag, 42.0);
        assert!(v.unit.is_dimensionless());
    }

    #[test]
    fn mph_to_base() {
        let v = eval_str("42 mph");
        assert!(approx_eq(v.mag, 42.0 * 1609.344 / 3600.0, 1e-9));
        assert_eq!(v.unit, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
    }

    #[test]
    fn add_same_unit() {
        let v = eval_str("1 m + 2 m");
        assert!(approx_eq(v.mag, 3.0, 1e-9));
    }

    #[test]
    fn add_unit_mismatch() {
        let mut eng = Engine::new();
        let e = eng.eval("1 m + 1 s").unwrap_err();
        assert!(matches!(e, Error::UnitMismatch { .. }));
    }

    #[test]
    fn pace_golden() {
        let v = eval_str("26.2 miles * 9:09 min/mile");
        assert!(approx_eq(v.mag, 14383.8, 1e-1), "got mag {}", v.mag);
        assert_eq!(v.unit, Unit::SECONDS);
    }

    #[test]
    fn assignment_returns_value() {
        let mut eng = Engine::new();
        match eng.eval("x = 10 m").unwrap() {
            Answer::Assigned { name, value } => {
                assert_eq!(name, "x");
                assert_eq!(value.mag, 10.0);
                assert_eq!(value.unit, Unit::METERS);
            }
            _ => panic!("expected Assigned"),
        }
        let v = match eng.eval("x / (3 s)").unwrap() {
            Answer::Bare(v) => v,
            _ => panic!(),
        };
        assert!(approx_eq(v.mag, 10.0 / 3.0, 1e-9));
        assert_eq!(v.unit, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
    }

    #[test]
    fn case_insensitive_variables() {
        let mut eng = Engine::new();
        eng.eval("X = 42 mph").unwrap();
        let r = eng.eval("x in m/s").unwrap();
        assert_eq!(crate::format::format(&r), "18.77568 m/s");
        eng.eval("MyVar = 5").unwrap();
        eng.eval("MYVAR = 10").unwrap();
        let r = eng.eval("myvar").unwrap();
        assert_eq!(crate::format::format(&r), "10");
    }

    #[test]
    fn ans_tracks_last_result() {
        let mut eng = Engine::new();
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "0");
        eng.eval("3 + 4").unwrap();
        let r = eng.eval("Ans * 2").unwrap();
        assert_eq!(crate::format::format(&r), "14");
        eng.eval("y = 100 m").unwrap();
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "100 m");
        let r = eng.eval("ANS + 50 m").unwrap();
        assert_eq!(crate::format::format(&r), "150 m");
    }

    #[test]
    fn reset_clears_vars_keeps_registry() {
        let mut eng = Engine::new();
        eng.eval("x = 42 mph").unwrap();
        eng.eval("y = x * 2").unwrap();
        eng.reset();
        assert!(matches!(eng.eval("x"), Err(Error::UndefinedVar(_))));
        assert_eq!(crate::format::format(&eng.eval("Ans").unwrap()), "0");
        assert!(eng.eval("1 mph").is_ok());
    }

    #[test]
    fn ans_not_updated_on_error() {
        let mut eng = Engine::new();
        eng.eval("7").unwrap();
        let _ = eng.eval("1 m + 1 s");
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "7");
    }

    #[test]
    fn as_is_alias_for_in() {
        let mut eng = Engine::new();
        let r = eng.eval("0.5 days as hms").unwrap();
        assert_eq!(crate::format::format(&r), "12:00:00.00");
        let r = eng.eval("14 hours as seconds").unwrap();
        assert_eq!(crate::format::format(&r), "50400 seconds");
    }

    #[test]
    fn short_symbols_dont_pluralize() {
        let mut eng = Engine::new();
        for bogus in ["gs", "Ks"] {
            let e = eng.eval(&format!("1 {bogus}")).unwrap_err();
            assert!(
                matches!(e, Error::UndefinedVar(_) | Error::UnknownUnit(_)),
                "{bogus} should not resolve, got {e:?}"
            );
        }
        let r = eng.eval("1 ms in millisecond").unwrap();
        assert_eq!(crate::format::format(&r), "1 millisecond");
        for ok in ["meters", "miles", "seconds", "hours", "days"] {
            assert!(eng.eval(&format!("1 {ok}")).is_ok(), "{ok} should resolve");
        }
    }

    #[test]
    fn in_hms_forces_hms_rendering() {
        let mut eng = Engine::new();
        let r = eng.eval("42 hours in hms").unwrap();
        assert_eq!(crate::format::format(&r), "42:00:00.00");
        let r = eng.eval("3661 s in hms").unwrap();
        assert_eq!(crate::format::format(&r), "01:01:01.00");
        eng.eval("x = 14 hours").unwrap();
        let _ = eng.eval("x in hms");
        let r = eng.eval("x").unwrap();
        assert_eq!(crate::format::format(&r), "14 hours");
        assert!(matches!(eng.eval("5 km in hms"), Err(Error::DimError(_))));
    }

    #[test]
    fn explicit_in_seconds_overrides_hms() {
        let mut eng = Engine::new();
        let r = eng.eval("14 hours in seconds").unwrap();
        assert_eq!(crate::format::format(&r), "50400 seconds");
        let r = eng.eval("14 hours in s").unwrap();
        assert_eq!(crate::format::format(&r), "50400 s");
        let r = eng.eval("9:09").unwrap();
        assert_eq!(crate::format::format(&r), "09:09.00");
        let r = eng.eval("9:09 + 9:09").unwrap();
        assert_eq!(crate::format::format(&r), "18:18.00");
    }

    #[test]
    fn convert() {
        let mut eng = Engine::new();
        let answer = eng.eval("3600 s in hr").unwrap();
        assert_eq!(crate::format::format(&answer), "1 hr");
    }

    // ─── temperature tests (the new spec) ─────────────────────────────────

    #[test]
    fn celsius_storage_in_kelvin() {
        let v = eval_str("25 degC");
        assert!((v.mag - 298.15).abs() < 1e-9, "got mag {}", v.mag);
        assert_eq!(v.unit, Unit::KELVIN);
        assert!(v.is_absolute_temp);
        assert!(!v.render_as_delta);
    }

    #[test]
    fn fahrenheit_storage_in_kelvin() {
        // 100 °F = 310.928 K
        let v = eval_str("100 degF");
        assert!((v.mag - 310.92777777778).abs() < 1e-6, "got mag {}", v.mag);
        assert_eq!(v.unit, Unit::KELVIN);
        assert!(v.is_absolute_temp);
    }

    #[test]
    fn celsius_to_fahrenheit() {
        let mut eng = Engine::new();
        let r = eng.eval("100 degC in degF").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("212"), "100°C → 212°F, got {s}");
        let r = eng.eval("0 degC in degF").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("32"), "0°C → 32°F, got {s}");
    }

    #[test]
    fn fahrenheit_to_celsius() {
        let mut eng = Engine::new();
        let r = eng.eval("100 degF in degC").unwrap();
        let s = crate::format::format(&r);
        // 100 °F = 37.777... °C
        assert!(s.starts_with("37.77"), "100°F → ~37.78°C, got {s}");
    }

    #[test]
    fn celsius_to_kelvin_preserves_absoluteness() {
        let v = eval_str("25 degC in K");
        assert_eq!(v.unit, Unit::KELVIN);
        assert!((v.mag - 298.15).abs() < 1e-9);
        // The keystone invariant: in K does NOT erase is_absolute_temp.
        assert!(v.is_absolute_temp,
                "25 degC in K must remain absolute — that's the whole point");
    }

    #[test]
    fn add_two_absolutes_errors() {
        let mut eng = Engine::new();
        assert!(matches!(
            eng.eval("25 degC + 25 degC"),
            Err(Error::TempArithmetic(_))
        ));
        assert!(matches!(
            eng.eval("100 degF + 50 degC"),
            Err(Error::TempArithmetic(_))
        ));
    }

    #[test]
    fn back_door_via_kelvin_still_errors() {
        // The exact case the spec is designed to prevent. After `in K`,
        // unit == K and display == K, but is_absolute_temp is still true,
        // so addition still errors. NEVER tempt a future reader to "optimize"
        // by inferring linearity from unit == K.
        let mut eng = Engine::new();
        assert!(matches!(
            eng.eval("(25 degC in K) + (25 degC in K)"),
            Err(Error::TempArithmetic(_))
        ));
    }

    #[test]
    fn abs_minus_abs_yields_delta_in_kelvin() {
        let mut eng = Engine::new();
        let r = eng.eval("30 degC - 25 degC").unwrap();
        assert_eq!(crate::format::format(&r), "5 K");
        // Internally: not absolute, render_as_delta = true.
        let v = match r {
            Answer::Bare(v) => v,
            _ => panic!(),
        };
        assert!(!v.is_absolute_temp);
        assert!(v.render_as_delta);
        assert_eq!(v.unit, Unit::KELVIN);
        assert!((v.mag - 5.0).abs() < 1e-9);
    }

    #[test]
    fn abs_plus_delta_yields_abs() {
        let mut eng = Engine::new();
        // 25 degC + 5 K = 303.15 K, displayed in degC = 30 °C
        let r = eng.eval("25 degC + 5 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("30") && s.contains("°C") || s.contains("degC"),
                "got {s}");
    }

    #[test]
    fn abs_minus_delta_yields_abs() {
        let mut eng = Engine::new();
        // 25 degC - 5 K = 293.15 K = 20 °C
        let r = eng.eval("25 degC - 5 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("20"), "got {s}");
    }

    #[test]
    fn delta_minus_abs_errors() {
        let mut eng = Engine::new();
        assert!(matches!(
            eng.eval("5 K - 25 degC"),
            Err(Error::TempArithmetic(_))
        ));
    }

    #[test]
    fn abs_times_scalar_errors() {
        let mut eng = Engine::new();
        assert!(matches!(
            eng.eval("25 degC * 2"),
            Err(Error::TempArithmetic(_))
        ));
        assert!(matches!(
            eng.eval("100 degF / 2"),
            Err(Error::TempArithmetic(_))
        ));
        assert!(matches!(
            eng.eval("25 degC ^ 2"),
            Err(Error::TempArithmetic(_))
        ));
    }

    #[test]
    fn kelvin_arithmetic_unchanged() {
        let mut eng = Engine::new();
        let r = eng.eval("5 K + 5 K").unwrap();
        assert_eq!(crate::format::format(&r), "10 K");
        let r = eng.eval("5 K * 2").unwrap();
        assert_eq!(crate::format::format(&r), "10 K");
    }

    #[test]
    fn delta_converted_to_affine_uses_delta_marker() {
        let mut eng = Engine::new();
        // 30 °C - 25 °C = 5 K (delta). `in degC` should keep the delta
        // semantics and add a Δ marker.
        let r = eng.eval("(30 degC - 25 degC) in degC").unwrap();
        let s = crate::format::format(&r);
        assert!(s.contains("Δ"), "expected Δ marker in {s}");
        // 5 K in degC: linear → affine, delta display.
        let r = eng.eval("5 K in degC").unwrap();
        let s = crate::format::format(&r);
        assert!(s.contains("Δ"), "expected Δ marker in {s}");
    }

    #[test]
    fn delta_times_scalar_works() {
        let mut eng = Engine::new();
        let r = eng.eval("(30 degC - 25 degC) * 3").unwrap();
        // 5 K * 3 = 15 K
        assert_eq!(crate::format::format(&r), "15 K");
    }

    #[test]
    fn abs_added_to_zero_kelvin_unchanged() {
        // Edge case: 25 degC + 0 K = 25 degC absolute.
        let mut eng = Engine::new();
        let r = eng.eval("25 degC + 0 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("25"), "got {s}");
    }

    #[test]
    fn absolute_temp_chained_conversions_preserve_flag() {
        // 25 degC in K in degF should give 77 °F absolute.
        let mut eng = Engine::new();
        let r = eng.eval("(25 degC in K) in degF").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("77"), "25°C → K → °F should be ~77°F, got {s}");
        // And another addition should still error if we add another absolute.
        // (Note: this isn't possible to test on Ans because abs+abs syntax
        //  would error before chaining; we trust the flag propagation tests above.)
    }

    #[test]
    fn negation_preserves_absoluteness() {
        // -(25 degC) is conceptually a negative absolute reading. Eval keeps
        // is_absolute_temp = true on negation.
        let v = eval_str("-(25 degC)");
        assert!(v.is_absolute_temp);
        assert!((v.mag - (-298.15)).abs() < 1e-9);
    }

    #[test]
    fn rankine_still_linear() {
        // Rankine has offset 0 (it shares Kelvin's zero point, just scales by 5/9).
        // Should load as a plain linear unit, no affine semantics.
        let v = eval_str("1 rankine");
        assert!(!v.is_absolute_temp, "rankine has offset 0; should be linear");
    }
}
