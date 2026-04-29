use crate::error::Error;
use crate::unit::Unit;
use crate::value::Value;

pub fn call(name: &str, args: Vec<Value>) -> Result<Value, Error> {
    match name {
        "sqrt" => {
            check_arity(name, &args, 1)?;
            let v = &args[0];
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
            Ok(Value::new(args[0].mag.abs(), args[0].unit))
        }
        "min" => {
            check_arity(name, &args, 2)?;
            require_same_unit(name, &args[0], &args[1])?;
            Ok(if args[0].mag <= args[1].mag {
                args[0].clone()
            } else {
                args[1].clone()
            })
        }
        "max" => {
            check_arity(name, &args, 2)?;
            require_same_unit(name, &args[0], &args[1])?;
            Ok(if args[0].mag >= args[1].mag {
                args[0].clone()
            } else {
                args[1].clone()
            })
        }
        _ => Err(Error::UnknownFn(name.to_string())),
    }
}

fn check_arity(name: &str, args: &[Value], expected: usize) -> Result<(), Error> {
    if args.len() != expected {
        return Err(Error::Arity {
            name: name.to_string(),
            expected,
            got: args.len(),
        });
    }
    Ok(())
}

fn require_same_unit(_name: &str, a: &Value, b: &Value) -> Result<(), Error> {
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
