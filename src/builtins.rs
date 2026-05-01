use crate::error::Error;
use crate::unit::Unit;
use crate::value::{LinearValue, Value};

pub fn call(name: &str, args: Vec<Value>) -> Result<Value, Error> {
    // sqrt/abs/min/max only operate on linear scalar values. Reject Instant
    // and Period inputs with a clear error.
    let args: Vec<LinearValue> = args
        .into_iter()
        .map(|v| match v {
            Value::Linear(l) => Ok(l),
            Value::Instant(_) | Value::Period(_) => Err(Error::TimeArithmetic(format!(
                "{name} doesn't accept dates or periods"
            ))),
        })
        .collect::<Result<_, _>>()?;

    match name {
        "sqrt" => {
            check_arity(name, &args, 1)?;
            let v = &args[0];
            if v.is_absolute_temp {
                return Err(Error::TempArithmetic(
                    "sqrt of an absolute temperature is meaningless".to_string(),
                ));
            }
            let u = &v.unit;
            let exps = [u.m, u.s, u.kg, u.a, u.k, u.mol, u.cd];
            if exps.iter().any(|e| e % 2 != 0) {
                return Err(Error::DimError(format!(
                    "sqrt requires even unit exponents, got {}",
                    if u.is_dimensionless() { "dimensionless".to_string() } else { u.render() }
                )));
            }
            Ok(Value::new(
                v.mag.sqrt(),
                Unit {
                    m: u.m / 2,
                    s: u.s / 2,
                    kg: u.kg / 2,
                    a: u.a / 2,
                    k: u.k / 2,
                    mol: u.mol / 2,
                    cd: u.cd / 2,
                },
            ))
        }
        "abs" => {
            check_arity(name, &args, 1)?;
            Ok(Value::Linear(LinearValue {
                mag: args[0].mag.abs(),
                ..args[0].clone()
            }))
        }
        "min" => {
            check_arity(name, &args, 2)?;
            require_same_unit(name, &args[0], &args[1])?;
            Ok(Value::Linear(if args[0].mag <= args[1].mag {
                args[0].clone()
            } else {
                args[1].clone()
            }))
        }
        "max" => {
            check_arity(name, &args, 2)?;
            require_same_unit(name, &args[0], &args[1])?;
            Ok(Value::Linear(if args[0].mag >= args[1].mag {
                args[0].clone()
            } else {
                args[1].clone()
            }))
        }
        _ => Err(Error::UnknownFn(name.to_string())),
    }
}

fn check_arity(name: &str, args: &[LinearValue], expected: usize) -> Result<(), Error> {
    if args.len() != expected {
        return Err(Error::Arity {
            name: name.to_string(),
            expected,
            got: args.len(),
        });
    }
    Ok(())
}

fn require_same_unit(_name: &str, a: &LinearValue, b: &LinearValue) -> Result<(), Error> {
    if a.unit != b.unit {
        return Err(Error::UnitMismatch {
            left: render_unit(&a.unit),
            right: render_unit(&b.unit),
        });
    }
    Ok(())
}

fn render_unit(u: &Unit) -> String {
    if u.is_dimensionless() {
        "dimensionless".to_string()
    } else {
        u.render()
    }
}
