use std::collections::HashMap;

use crate::ast::*;
use crate::builtins;
use crate::error::Error;
use crate::unit::{Unit, UnitRegistry};
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
            let (unit, factor) = env.registry.resolve(u)?;
            Ok(Value::with_display(n * factor, unit, u.clone()))
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
                Ok((unit, factor)) => Ok(Value::with_display(factor, unit, atom)),
                Err(Error::OffsetUnit(_)) => Err(Error::OffsetUnit(name.clone())),
                Err(_) => Err(Error::UndefinedVar(name.clone())),
            }
        }
        Expr::Unary(UnaryOp::Neg, inner) => {
            let v = eval_expr(env, inner)?;
            Ok(Value {
                mag: -v.mag,
                unit: v.unit,
                display: v.display,
            })
        }
        Expr::Binary(a, op, b) => {
            let va = eval_expr(env, a)?;
            let vb = eval_expr(env, b)?;
            match op {
                Op::Add => {
                    require_same_unit(&va, &vb)?;
                    Ok(Value {
                        mag: va.mag + vb.mag,
                        unit: va.unit,
                        display: va.display.or(vb.display),
                    })
                }
                Op::Sub => {
                    require_same_unit(&va, &vb)?;
                    Ok(Value {
                        mag: va.mag - vb.mag,
                        unit: va.unit,
                        display: va.display.or(vb.display),
                    })
                }
                Op::Mul => Ok(Value {
                    mag: va.mag * vb.mag,
                    unit: va.unit.mul(&vb.unit),
                    display: combine_display(va.display, vb.display, true),
                }),
                Op::Div => Ok(Value {
                    mag: va.mag / vb.mag,
                    unit: va.unit.div(&vb.unit),
                    display: combine_display(va.display, vb.display, false),
                }),
                Op::Pow => {
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
            }
        }
        Expr::Call(name, args) => {
            let mut vs = Vec::with_capacity(args.len());
            for a in args {
                vs.push(eval_expr(env, a)?);
            }
            builtins::call(name, vs)
        }
        Expr::Convert(inner, target) => {
            let v = eval_expr(env, inner)?;
            let (target_unit, _factor) = env.registry.resolve(target)?;
            if v.unit != target_unit {
                return Err(Error::UnitMismatch {
                    left: render_unit(&v.unit),
                    right: render_unit(&target_unit),
                });
            }
            // Magnitude stays in SI base; conversion is purely a display change.
            // The formatter divides by the target's factor when rendering.
            Ok(Value::with_display(v.mag, v.unit, target.clone()))
        }
    }
}

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
                // `5 / mph` would have display `1/mph`; not worth expressing as
                // a UnitExpr right now, so the formatter falls back to base.
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
        // 26.2 miles * 9:09 min/mile → 14383.8 s = 03:59:43.8
        // 9:09 min/mile lowers via time-component factoring to Number(9.15, min/mile),
        // which resolves to 9.15 * 60/1609.344 = 0.3411 s/m.
        // 26.2 mile = 42164.8 m. 42164.8 m * 0.3411 s/m = 14383.8 s.
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
        // x is now bound; reuse it. With units-as-values + left-assoc *, the
        // divisor must be parenthesized: `x / 3 s` is `(x/3) * s`, not `x / (3*s)`.
        let v = match eng.eval("x / (3 s)").unwrap() {
            Answer::Bare(v) => v,
            _ => panic!(),
        };
        assert!(approx_eq(v.mag, 10.0 / 3.0, 1e-9));
        assert_eq!(v.unit, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
    }

    #[test]
    fn offset_temperature_units_excluded() {
        // celsius / fahrenheit / degC etc. are deliberately not registered —
        // they would silently give wrong answers under linear conversion.
        // The user gets a specific error pointing them at kelvin instead.
        let mut eng = Engine::new();
        for name in ["celsius", "fahrenheit", "degC", "degF", "degree_Celsius"] {
            let e = eng.eval(&format!("1 {name}")).unwrap_err();
            assert!(
                matches!(e, Error::OffsetUnit(_)),
                "expected OffsetUnit for {name}, got {e:?}"
            );
        }
        // kelvin and rankine (linear-equivalent, offset 0) are fine.
        for name in ["kelvin", "K", "rankine"] {
            let r = eng.eval(&format!("1 {name}"));
            assert!(r.is_ok(), "{name} should resolve, got {r:?}");
        }
    }

    #[test]
    fn case_insensitive_variables() {
        let mut eng = Engine::new();
        eng.eval("X = 42 mph").unwrap();
        // Same var, different case in the lookup.
        let r = eng.eval("x in m/s").unwrap();
        assert_eq!(crate::format::format(&r), "18.77568 m/s");
        // Reassigning with a different case overwrites the same binding.
        eng.eval("MyVar = 5").unwrap();
        eng.eval("MYVAR = 10").unwrap();
        let r = eng.eval("myvar").unwrap();
        assert_eq!(crate::format::format(&r), "10");
    }

    #[test]
    fn ans_tracks_last_result() {
        let mut eng = Engine::new();
        // Fresh engine: Ans defaults to 0.
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "0");
        // After a bare expression, Ans is the result.
        eng.eval("3 + 4").unwrap();
        let r = eng.eval("Ans * 2").unwrap();
        assert_eq!(crate::format::format(&r), "14");
        // Assignment also updates Ans.
        eng.eval("y = 100 m").unwrap();
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "100 m");
        // Ans is itself case-insensitive.
        let r = eng.eval("ANS + 50 m").unwrap();
        assert_eq!(crate::format::format(&r), "150 m");
    }

    #[test]
    fn ans_not_updated_on_error() {
        let mut eng = Engine::new();
        eng.eval("7").unwrap();
        let _ = eng.eval("1 m + 1 s"); // unit mismatch error
        let r = eng.eval("Ans").unwrap();
        assert_eq!(crate::format::format(&r), "7");
    }

    #[test]
    fn convert() {
        // After conversion, magnitude stays in SI base — only display changes.
        // The user-visible result is rendered as "1 hr".
        let mut eng = Engine::new();
        let answer = eng.eval("3600 s in hr").unwrap();
        assert_eq!(crate::format::format(&answer), "1 hr");
    }
}
