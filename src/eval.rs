use std::collections::HashMap;

use crate::ast::*;
use crate::builtins;
use crate::calendar;
use crate::error::Error;
use crate::unit::{AtomKind, Unit, UnitRegistry};
use crate::value::{LinearValue, PeriodValue, Value};

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
        env.set("Ans", Value::dimensionless(0.0));
        env
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.vars.get(&name.to_ascii_lowercase())
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.vars.insert(name.to_ascii_lowercase(), value);
    }

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
        Expr::Number(n, Some(u)) => eval_number_with_unit(env, *n, u),
        Expr::DateLiteral(y, m, d) => calendar::make_instant(*y, *m, *d).map(Value::Instant),
        Expr::Var(name) => eval_var(env, name),
        Expr::Unary(UnaryOp::Neg, inner) => negate(eval_expr(env, inner)?),
        Expr::Binary(a, op, b) => {
            // Affine-literal shortcut: `25 degC` parses as `25 * Var(degC)`
            // due to units-as-values + implicit multiplication. Catch that
            // pattern before normal eval so we apply the offset and produce
            // an absolute literal instead of trying to multiply by an
            // already-absolute Kelvin value.
            if let Op::Mul = op {
                if let Some(v) = try_affine_literal(env, a, b) {
                    return Ok(v);
                }
                // Period-literal shortcut: `5 cal_days`, `1 month`, etc.
                if let Some(v) = try_period_literal(env, a, b) {
                    return Ok(v);
                }
            }
            let va = eval_expr(env, a)?;
            let vb = eval_expr(env, b)?;
            apply_binary(*op, va, vb)
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

fn eval_number_with_unit(env: &Env, n: f64, u: &UnitExpr) -> Result<Value, Error> {
    let r = env.registry.resolve(u)?;
    if let Some(atom) = r.atom {
        match atom.kind {
            AtomKind::AffineTemp => {
                let mag = n * atom.factor + atom.offset;
                return Ok(Value::absolute_temp(mag, u.clone()));
            }
            AtomKind::Period { years, months, days } => {
                if !is_integer(n) {
                    return Err(Error::TimeArithmetic(
                        "period literals require an integer count".to_string(),
                    ));
                }
                let k = n as i32;
                return Ok(Value::period(years * k, months * k, days * k));
            }
            AtomKind::Linear => {}
        }
    }
    Ok(Value::with_display(n * r.factor, r.unit, u.clone()))
}

fn eval_var(env: &Env, name: &str) -> Result<Value, Error> {
    if let Some(v) = env.get(name) {
        return Ok(v.clone());
    }
    let atom = UnitExpr::Atom(name.to_string(), 1);
    match env.registry.resolve(&atom) {
        Ok(r) => {
            if let Some(def) = r.atom {
                match def.kind {
                    AtomKind::AffineTemp => {
                        let mag = def.factor + def.offset;
                        return Ok(Value::absolute_temp(mag, atom));
                    }
                    AtomKind::Period { years, months, days } => {
                        return Ok(Value::period(years, months, days));
                    }
                    AtomKind::Linear => {}
                }
            }
            Ok(Value::with_display(r.factor, r.unit, atom))
        }
        Err(_) => Err(Error::UndefinedVar(name.to_string())),
    }
}

fn negate(v: Value) -> Result<Value, Error> {
    match v {
        Value::Linear(l) => Ok(Value::Linear(LinearValue { mag: -l.mag, ..l })),
        Value::Period(p) => Ok(Value::Period(PeriodValue {
            years: -p.years,
            months: -p.months,
            days: -p.days,
        })),
        Value::Instant(_) => Err(Error::TimeArithmetic(
            "cannot negate an instant".to_string(),
        )),
    }
}

/// Detect `Number(n) * Var(name)` where `name` resolves to an affine
/// atom. This is how the grammar represents `25 degC` (because we use
/// units-as-values with implicit multiplication).
fn try_affine_literal(env: &Env, a: &Expr, b: &Expr) -> Option<Value> {
    let (n, name) = number_var_pair(a, b)?;
    if env.get(name).is_some() {
        return None;
    }
    let r = env.registry.resolve(&UnitExpr::Atom(name.clone(), 1)).ok()?;
    let atom = r.atom?;
    if !matches!(atom.kind, AtomKind::AffineTemp) {
        return None;
    }
    let mag = n * atom.factor + atom.offset;
    Some(Value::absolute_temp(mag, UnitExpr::Atom(name.clone(), 1)))
}

/// Detect `Number(n) * Var(name)` where `name` resolves to a Period atom
/// (`cal_days`, `month`, `years`, etc.). Mirrors the affine-literal trick:
/// the grammar treats `5 cal_days` as `5 * Var(cal_days)`, but semantically
/// it's a period literal of 5 calendar days.
fn try_period_literal(env: &Env, a: &Expr, b: &Expr) -> Option<Value> {
    let (n, name) = number_var_pair(a, b)?;
    if env.get(name).is_some() {
        return None;
    }
    let r = env.registry.resolve(&UnitExpr::Atom(name.clone(), 1)).ok()?;
    let atom = r.atom?;
    let AtomKind::Period { years, months, days } = atom.kind else {
        return None;
    };
    if !is_integer(n) {
        // Non-integer scaling on a period is hard-error per spec; surface
        // it via the actual arithmetic path so the user gets a TimeArithmetic
        // error rather than a silent fallthrough.
        return None;
    }
    let k = n as i32;
    Some(Value::period(years * k, months * k, days * k))
}

fn number_var_pair<'a>(a: &'a Expr, b: &'a Expr) -> Option<(f64, &'a String)> {
    match (a, b) {
        (Expr::Number(n, None), Expr::Var(name)) => Some((*n, name)),
        (Expr::Var(name), Expr::Number(n, None)) => Some((*n, name)),
        _ => None,
    }
}

fn is_integer(n: f64) -> bool {
    n.is_finite() && (n - n.round()).abs() < 1e-9
}

// ─── arithmetic dispatcher ─────────────────────────────────────────────────
//
// Per the time spec: branch on Value variant ONLY. Per the temperature spec:
// for Linear-Linear arithmetic, branch on `is_absolute_temp` ONLY. Never
// infer semantics from `unit` or `display`.

fn apply_binary(op: Op, va: Value, vb: Value) -> Result<Value, Error> {
    use Value::*;
    match (op, va, vb) {
        (Op::Add, Linear(la), Linear(lb)) => add_linear(la, lb),
        (Op::Sub, Linear(la), Linear(lb)) => sub_linear(la, lb),
        (Op::Mul, Linear(la), Linear(lb)) => mul_linear(la, lb),
        (Op::Div, Linear(la), Linear(lb)) => div_linear(la, lb),
        (Op::Pow, Linear(la), Linear(lb)) => pow_linear(la, lb),

        // Instant ± Duration → Instant.
        (Op::Add, Instant(i), Linear(l)) | (Op::Add, Linear(l), Instant(i)) => {
            calendar::instant_plus_duration(i, l).map(Value::Instant)
        }
        (Op::Sub, Instant(i), Linear(l)) => {
            calendar::instant_minus_duration(i, l).map(Value::Instant)
        }
        // Instant - Instant → Duration.
        (Op::Sub, Instant(a), Instant(b)) => Ok(calendar::instant_minus_instant(a, b)),

        // Instant + Period → Instant. (Spec calls this case calendar-aware.)
        (Op::Add, Instant(i), Period(p)) | (Op::Add, Period(p), Instant(i)) => {
            calendar::instant_plus_period(i, p).map(Value::Instant)
        }
        (Op::Sub, Instant(i), Period(p)) => {
            calendar::instant_minus_period(i, p).map(Value::Instant)
        }

        // Period ± Period → Period (component-wise, no normalization).
        (Op::Add, Period(a), Period(b)) => Ok(Value::Period(PeriodValue {
            years: a.years + b.years,
            months: a.months + b.months,
            days: a.days + b.days,
        })),
        (Op::Sub, Period(a), Period(b)) => Ok(Value::Period(PeriodValue {
            years: a.years - b.years,
            months: a.months - b.months,
            days: a.days - b.days,
        })),

        // Period × scalar (integer only) → Period.
        (Op::Mul, Period(p), Linear(l)) | (Op::Mul, Linear(l), Period(p)) => {
            period_times_scalar(p, l)
        }

        // Everything else is an error per the spec.
        (op, va, vb) => Err(time_arith_err(op, &va, &vb)),
    }
}

fn time_arith_err(op: Op, va: &Value, vb: &Value) -> Error {
    let kind = |v: &Value| match v {
        Value::Linear(_) => "duration/scalar",
        Value::Instant(_) => "instant",
        Value::Period(_) => "period",
    };
    let op_name = match op {
        Op::Add => "add",
        Op::Sub => "subtract",
        Op::Mul => "multiply",
        Op::Div => "divide",
        Op::Pow => "exponentiate",
    };
    Error::TimeArithmetic(format!(
        "cannot {op_name} {} and {}",
        kind(va),
        kind(vb)
    ))
}

fn period_times_scalar(p: PeriodValue, l: LinearValue) -> Result<Value, Error> {
    if !l.unit.is_dimensionless() {
        return Err(Error::TimeArithmetic(
            "period can only be multiplied by a dimensionless integer".to_string(),
        ));
    }
    if !is_integer(l.mag) {
        return Err(Error::TimeArithmetic(
            "period scaling requires an integer".to_string(),
        ));
    }
    let k = l.mag as i32;
    Ok(Value::Period(PeriodValue {
        years: p.years * k,
        months: p.months * k,
        days: p.days * k,
    }))
}

// ─── linear arithmetic (the existing temperature-aware logic) ──────────────

fn add_linear(la: LinearValue, lb: LinearValue) -> Result<Value, Error> {
    match (la.is_absolute_temp, lb.is_absolute_temp) {
        (true, true) => Err(Error::TempArithmetic(
            "cannot add two absolute temperatures".to_string(),
        )),
        (true, false) | (false, true) => {
            require_same_unit(&la, &lb)?;
            let (abs, _other) = if la.is_absolute_temp { (&la, &lb) } else { (&lb, &la) };
            Ok(Value::Linear(LinearValue {
                mag: la.mag + lb.mag,
                unit: la.unit,
                display: abs.display.clone(),
                is_absolute_temp: true,
                ..Default::default()
            }))
        }
        (false, false) => {
            require_same_unit(&la, &lb)?;
            Ok(Value::Linear(LinearValue {
                mag: la.mag + lb.mag,
                unit: la.unit,
                display: la.display.or(lb.display),
                ..Default::default()
            }))
        }
    }
}

fn sub_linear(la: LinearValue, lb: LinearValue) -> Result<Value, Error> {
    match (la.is_absolute_temp, lb.is_absolute_temp) {
        (true, true) => {
            require_same_unit(&la, &lb)?;
            Ok(Value::Linear(LinearValue {
                mag: la.mag - lb.mag,
                unit: la.unit,
                display: None,
                render_as_delta: true,
                ..Default::default()
            }))
        }
        (true, false) => {
            require_same_unit(&la, &lb)?;
            Ok(Value::Linear(LinearValue {
                mag: la.mag - lb.mag,
                unit: la.unit,
                display: la.display,
                is_absolute_temp: true,
                ..Default::default()
            }))
        }
        (false, true) => Err(Error::TempArithmetic(
            "cannot subtract an absolute temperature from a delta".to_string(),
        )),
        (false, false) => {
            require_same_unit(&la, &lb)?;
            Ok(Value::Linear(LinearValue {
                mag: la.mag - lb.mag,
                unit: la.unit,
                display: la.display.or(lb.display),
                ..Default::default()
            }))
        }
    }
}

fn mul_linear(la: LinearValue, lb: LinearValue) -> Result<Value, Error> {
    if la.is_absolute_temp || lb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    Ok(Value::Linear(LinearValue {
        mag: la.mag * lb.mag,
        unit: la.unit.mul(&lb.unit),
        display: combine_display(la.display, lb.display, true),
        ..Default::default()
    }))
}

fn div_linear(la: LinearValue, lb: LinearValue) -> Result<Value, Error> {
    if la.is_absolute_temp || lb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    Ok(Value::Linear(LinearValue {
        mag: la.mag / lb.mag,
        unit: la.unit.div(&lb.unit),
        display: combine_display(la.display, lb.display, false),
        ..Default::default()
    }))
}

fn pow_linear(la: LinearValue, lb: LinearValue) -> Result<Value, Error> {
    if la.is_absolute_temp || lb.is_absolute_temp {
        return Err(Error::TempArithmetic(
            "absolute temperatures cannot be scaled".to_string(),
        ));
    }
    if !lb.unit.is_dimensionless() {
        return Err(Error::DimError(
            "exponent must be dimensionless".to_string(),
        ));
    }
    let n = lb.mag;
    let int_exp = n.round() as i32;
    let is_int = (n - int_exp as f64).abs() < 1e-9;
    let unit = if is_int {
        la.unit.pow(int_exp)
    } else if la.unit.is_dimensionless() {
        la.unit
    } else {
        return Err(Error::DimError(
            "non-integer power requires a dimensionless base".to_string(),
        ));
    };
    Ok(Value::new(la.mag.powf(n), unit))
}

// ─── conversion ────────────────────────────────────────────────────────────

fn convert_value(env: &mut Env, inner: &Expr, target: &UnitExpr) -> Result<Value, Error> {
    let v = eval_expr(env, inner)?;
    // Most conversions only apply to linear values. Instants and Periods get
    // a clear error in v1 (no anchored period↔duration conversion yet).
    let l = match v {
        Value::Linear(l) => l,
        Value::Instant(_) => {
            return Err(Error::TimeArithmetic(
                "cannot convert an instant via `in`".to_string(),
            ));
        }
        Value::Period(_) => {
            return Err(Error::TimeArithmetic(
                "cannot convert a period to a unit (no anchor)".to_string(),
            ));
        }
    };

    if matches!(target, UnitExpr::Atom(name, 1) if name == "hms") {
        if !l.unit.is_pure_time() {
            return Err(Error::DimError(format!(
                "`in hms` requires a time value, got {}",
                render_unit(&l.unit)
            )));
        }
        return Ok(Value::with_force_hms(l.mag, l.unit));
    }

    let r = env.registry.resolve(target)?;
    if l.unit != r.unit {
        return Err(Error::UnitMismatch {
            left: render_unit(&l.unit),
            right: render_unit(&r.unit),
        });
    }
    let target_is_affine = r
        .atom
        .as_ref()
        .map(|a| matches!(a.kind, AtomKind::AffineTemp))
        .unwrap_or(false);

    if target_is_affine {
        Ok(Value::Linear(LinearValue {
            mag: l.mag,
            unit: l.unit,
            display: Some(target.clone()),
            display_explicit: true,
            force_hms: false,
            is_absolute_temp: l.is_absolute_temp,
            render_as_delta: !l.is_absolute_temp || l.render_as_delta,
        }))
    } else {
        // Critical: is_absolute_temp propagates UNCHANGED. `in K` on an
        // absolute keeps the flag — preventing the `(25 degC in K) +
        // (25 degC in K)` back-door.
        Ok(Value::Linear(LinearValue {
            mag: l.mag,
            unit: l.unit,
            display: Some(target.clone()),
            display_explicit: true,
            force_hms: false,
            is_absolute_temp: l.is_absolute_temp,
            render_as_delta: l.render_as_delta,
        }))
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

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

fn require_same_unit(a: &LinearValue, b: &LinearValue) -> Result<(), Error> {
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

// keep Resolved name imported for use in dependents; silence unused warn
#[allow(dead_code)]
fn _resolved_signature(_: &crate::unit::Resolved) {}

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

    fn eval_linear(s: &str) -> LinearValue {
        match eval_str(s) {
            Value::Linear(l) => l,
            other => panic!("expected Linear, got {other:?}"),
        }
    }

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn pure_number() {
        let v = eval_linear("42");
        assert_eq!(v.mag, 42.0);
        assert!(v.unit.is_dimensionless());
    }

    #[test]
    fn mph_to_base() {
        let v = eval_linear("42 mph");
        assert!(approx_eq(v.mag, 42.0 * 1609.344 / 3600.0, 1e-9));
        assert_eq!(v.unit, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
    }

    #[test]
    fn add_same_unit() {
        let v = eval_linear("1 m + 2 m");
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
        let v = eval_linear("26.2 miles * 9:09 min/mile");
        assert!(approx_eq(v.mag, 14383.8, 1e-1), "got mag {}", v.mag);
        assert_eq!(v.unit, Unit::SECONDS);
    }

    #[test]
    fn assignment_returns_value() {
        let mut eng = Engine::new();
        match eng.eval("x = 10 m").unwrap() {
            Answer::Assigned { name, value } => {
                assert_eq!(name, "x");
                let l = value.as_linear().unwrap();
                assert_eq!(l.mag, 10.0);
                assert_eq!(l.unit, Unit::METERS);
            }
            _ => panic!("expected Assigned"),
        }
        let v = match eng.eval("x / (3 s)").unwrap() {
            Answer::Bare(v) => v,
            _ => panic!(),
        };
        let l = v.as_linear().unwrap();
        assert!(approx_eq(l.mag, 10.0 / 3.0, 1e-9));
        assert_eq!(l.unit, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
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

    // ─── temperature tests ─────────────────────────────────────────────────

    #[test]
    fn celsius_storage_in_kelvin() {
        let v = eval_linear("25 degC");
        assert!((v.mag - 298.15).abs() < 1e-9);
        assert_eq!(v.unit, Unit::KELVIN);
        assert!(v.is_absolute_temp);
        assert!(!v.render_as_delta);
    }

    #[test]
    fn fahrenheit_storage_in_kelvin() {
        let v = eval_linear("100 degF");
        assert!((v.mag - 310.92777777778).abs() < 1e-6);
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
        assert!(s.starts_with("37.77"), "100°F → ~37.78°C, got {s}");
    }

    #[test]
    fn celsius_to_kelvin_preserves_absoluteness() {
        let v = eval_linear("25 degC in K");
        assert_eq!(v.unit, Unit::KELVIN);
        assert!((v.mag - 298.15).abs() < 1e-9);
        assert!(v.is_absolute_temp,
                "25 degC in K must remain absolute — that's the whole point");
    }

    #[test]
    fn add_two_absolutes_errors() {
        let mut eng = Engine::new();
        assert!(matches!(eng.eval("25 degC + 25 degC"), Err(Error::TempArithmetic(_))));
        assert!(matches!(eng.eval("100 degF + 50 degC"), Err(Error::TempArithmetic(_))));
    }

    #[test]
    fn back_door_via_kelvin_still_errors() {
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
        let v = match r {
            Answer::Bare(v) => v,
            _ => panic!(),
        };
        let l = v.as_linear().unwrap();
        assert!(!l.is_absolute_temp);
        assert!(l.render_as_delta);
        assert_eq!(l.unit, Unit::KELVIN);
        assert!((l.mag - 5.0).abs() < 1e-9);
    }

    #[test]
    fn abs_plus_delta_yields_abs() {
        let mut eng = Engine::new();
        let r = eng.eval("25 degC + 5 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("30") && (s.contains("°C") || s.contains("degC")), "got {s}");
    }

    #[test]
    fn abs_minus_delta_yields_abs() {
        let mut eng = Engine::new();
        let r = eng.eval("25 degC - 5 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("20"), "got {s}");
    }

    #[test]
    fn delta_minus_abs_errors() {
        let mut eng = Engine::new();
        assert!(matches!(eng.eval("5 K - 25 degC"), Err(Error::TempArithmetic(_))));
    }

    #[test]
    fn abs_times_scalar_errors() {
        let mut eng = Engine::new();
        assert!(matches!(eng.eval("25 degC * 2"), Err(Error::TempArithmetic(_))));
        assert!(matches!(eng.eval("100 degF / 2"), Err(Error::TempArithmetic(_))));
        assert!(matches!(eng.eval("25 degC ^ 2"), Err(Error::TempArithmetic(_))));
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
        let r = eng.eval("(30 degC - 25 degC) in degC").unwrap();
        let s = crate::format::format(&r);
        assert!(s.contains("Δ"), "expected Δ marker in {s}");
        let r = eng.eval("5 K in degC").unwrap();
        let s = crate::format::format(&r);
        assert!(s.contains("Δ"), "expected Δ marker in {s}");
    }

    #[test]
    fn delta_times_scalar_works() {
        let mut eng = Engine::new();
        let r = eng.eval("(30 degC - 25 degC) * 3").unwrap();
        assert_eq!(crate::format::format(&r), "15 K");
    }

    #[test]
    fn abs_added_to_zero_kelvin_unchanged() {
        let mut eng = Engine::new();
        let r = eng.eval("25 degC + 0 K").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("25"), "got {s}");
    }

    #[test]
    fn absolute_temp_chained_conversions_preserve_flag() {
        let mut eng = Engine::new();
        let r = eng.eval("(25 degC in K) in degF").unwrap();
        let s = crate::format::format(&r);
        assert!(s.starts_with("77"), "25°C → K → °F should be ~77°F, got {s}");
    }

    #[test]
    fn negation_preserves_absoluteness() {
        let v = eval_linear("-(25 degC)");
        assert!(v.is_absolute_temp);
        assert!((v.mag - (-298.15)).abs() < 1e-9);
    }

    #[test]
    fn rankine_still_linear() {
        let v = eval_linear("1 rankine");
        assert!(!v.is_absolute_temp, "rankine has offset 0; should be linear");
    }

    // ─── date/time tests ───────────────────────────────────────────────────

    fn fmt(s: &str) -> String {
        let mut eng = Engine::new();
        crate::format::format(&eng.eval(s).unwrap())
    }

    fn fmt_err(s: &str) -> Error {
        let mut eng = Engine::new();
        eng.eval(s).unwrap_err()
    }

    #[test]
    fn date_literal_parses_to_instant() {
        let v = eval_str("2026-04-30");
        assert!(v.is_instant());
        assert_eq!(fmt("2026-04-30"), "2026-04-30");
    }

    #[test]
    fn date_plus_duration() {
        assert_eq!(fmt("2026-04-30 + 5 hours"), "2026-04-30T05:00:00Z");
        assert_eq!(fmt("2026-04-30 + 90 min"), "2026-04-30T01:30:00Z");
    }

    #[test]
    fn date_minus_duration() {
        assert_eq!(fmt("2026-04-30 - 1 hour"), "2026-04-29T23:00:00Z");
    }

    #[test]
    fn date_minus_date_yields_duration() {
        // The Linear result has unit=SECONDS, so HMS auto-format kicks in.
        assert_eq!(fmt("2026-04-30 - 2026-04-29"), "24:00:00.00");
        // User can convert to plain seconds:
        assert_eq!(fmt("(2026-04-30 - 2026-04-29) in seconds"), "86400 seconds");
    }

    #[test]
    fn date_plus_month_clamps_jan31() {
        assert_eq!(fmt("2026-01-31 + 1 month"), "2026-02-28");
    }

    #[test]
    fn date_minus_month_clamps_mar31() {
        assert_eq!(fmt("2026-03-31 - 1 month"), "2026-02-28");
    }

    #[test]
    fn period_plus_period_component_wise() {
        assert_eq!(fmt("1 month + 2 months"), "P3M");
        assert_eq!(fmt("1 year + 2 months + 3 cal_days"), "P1Y2M3D");
    }

    #[test]
    fn period_minus_period_allows_negative_components() {
        // `1 cal_day - 1 month` → Period { months: -1, days: 1 }.
        // Note: bare `1 day` is Duration, so we use `cal_day` for Period math.
        assert_eq!(fmt("1 cal_day - 1 month"), "P-1M1D");
    }

    #[test]
    fn duration_plus_period_errors() {
        assert!(matches!(fmt_err("5 hours + 1 month"), Error::TimeArithmetic(_)));
        assert!(matches!(fmt_err("1 month + 5 hours"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn instant_times_scalar_errors() {
        assert!(matches!(fmt_err("2026-04-30 * 2"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn instant_plus_instant_errors() {
        assert!(matches!(
            fmt_err("2026-04-30 + 2026-04-29"),
            Error::TimeArithmetic(_)
        ));
    }

    #[test]
    fn period_times_int_scalar_works() {
        assert_eq!(fmt("1 month * 3"), "P3M");
        assert_eq!(fmt("3 * 1 month"), "P3M");
    }

    #[test]
    fn period_times_float_errors() {
        assert!(matches!(fmt_err("1 month * 1.5"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn period_times_dimensioned_errors() {
        assert!(matches!(fmt_err("1 month * 5 m"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn five_days_still_duration() {
        // The disambiguation rule: bare `days` stays Linear, not Period.
        let v = eval_str("5 days");
        let l = v.as_linear().expect("5 days should be Linear (Duration)");
        assert!((l.mag - 5.0 * 86400.0).abs() < 1e-9);
        assert_eq!(l.unit, Unit::SECONDS);
    }

    #[test]
    fn cal_days_is_period() {
        let v = eval_str("5 cal_days");
        let p = v.as_period().expect("5 cal_days should be Period");
        assert_eq!(p.days, 5);
        assert_eq!(p.months, 0);
        assert_eq!(p.years, 0);
    }

    #[test]
    fn calendar_days_long_form_is_period() {
        let v = eval_str("5 calendar_days");
        assert!(v.is_period());
    }

    #[test]
    fn month_is_period_only() {
        // pint's linear month is overridden; bare `month` resolves to Period.
        let v = eval_str("1 month");
        let p = v.as_period().expect("1 month should be Period");
        assert_eq!(p.months, 1);
    }

    #[test]
    fn year_is_period_only() {
        let v = eval_str("1 year");
        let p = v.as_period().expect("1 year should be Period");
        assert_eq!(p.years, 1);
    }

    #[test]
    fn period_render_iso_8601() {
        assert_eq!(fmt("1 year"), "P1Y");
        assert_eq!(fmt("0 cal_days"), "P0D");
        assert_eq!(fmt("1 year + 2 months + 3 cal_days"), "P1Y2M3D");
    }

    #[test]
    fn date_arith_round_trip() {
        // (Feb 28) - (Feb 28) = 0 seconds, HMS format for <60s is "0.00".
        assert_eq!(fmt("(2026-01-31 + 1 month) - (2026-01-31 + 1 month)"), "0.00");
    }

    #[test]
    fn invalid_date_errors() {
        // 2026 isn't a leap year.
        assert!(matches!(fmt_err("2026-02-29"), Error::TimeArithmetic(_)));
        assert!(matches!(fmt_err("2026-13-01"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn instant_cannot_negate() {
        assert!(matches!(fmt_err("-(2026-04-30)"), Error::TimeArithmetic(_)));
    }

    #[test]
    fn period_negation_works() {
        // Negation of a period flips component signs.
        let v = eval_str("-(1 month)");
        let p = v.as_period().expect("Period");
        assert_eq!(p.months, -1);
    }

    #[test]
    fn instant_minus_period() {
        assert_eq!(fmt("2026-04-30 - 1 month"), "2026-03-30");
        assert_eq!(fmt("2026-03-30 - 2 cal_days"), "2026-03-28");
    }

    #[test]
    fn date_arith_preserves_canonical_pace_example() {
        // The Phase 1 canonical example must still work after the refactor.
        assert_eq!(fmt("26.2 miles * 9:09 min/mile"), "03:59:43.80");
    }

    #[test]
    fn date_literal_in_arithmetic_with_parens() {
        // Convoluted but spec-compliant: subtract a Duration from a date,
        // then add a Period to that.
        assert_eq!(fmt("(2026-04-30 - 1 hour) + 1 month"), "2026-05-29T23:00:00Z");
    }

    #[test]
    fn five_days_plus_five_days_still_works() {
        // Existing Duration arithmetic must keep working. The display
        // propagates `days`, so the result renders as "10 days" rather than
        // HMS — same as before the date/time work.
        assert_eq!(fmt("5 days + 5 days"), "10 days");
        // Round-trip via in-conversion also still works.
        assert_eq!(fmt("(5 days + 5 days) in hms"), "240:00:00.00");
    }

    #[test]
    fn cal_days_plus_cal_days() {
        assert_eq!(fmt("5 cal_days + 5 cal_days"), "P10D");
    }

    #[test]
    fn period_assignment_and_reuse() {
        let mut eng = Engine::new();
        eng.eval("p = 1 month").unwrap();
        let r = eng.eval("p + 2 months").unwrap();
        assert_eq!(crate::format::format(&r), "P3M");
    }

    #[test]
    fn instant_assignment_and_reuse() {
        let mut eng = Engine::new();
        eng.eval("d = 2026-04-30").unwrap();
        let r = eng.eval("d + 5 hours").unwrap();
        assert_eq!(crate::format::format(&r), "2026-04-30T05:00:00Z");
    }

    #[test]
    fn ans_with_dates() {
        let mut eng = Engine::new();
        eng.eval("2026-04-30").unwrap();
        let r = eng.eval("Ans + 5 hours").unwrap();
        assert_eq!(crate::format::format(&r), "2026-04-30T05:00:00Z");
    }

    #[test]
    fn sqrt_rejects_instant_and_period() {
        let mut eng = Engine::new();
        assert!(matches!(eng.eval("sqrt(2026-04-30)"), Err(Error::TimeArithmetic(_))));
        assert!(matches!(eng.eval("sqrt(1 month)"), Err(Error::TimeArithmetic(_))));
    }
}
