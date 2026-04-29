fn main() {
    let mut eng = poppop::Engine::new();
    for name in ["speed_of_light", "julian_year", "year", "light_year", "lightyear", "ly", "atm", "standard_atmosphere", "gallon", "us_gallon", "liter", "pi"] {
        let r = eng.eval(&format!("1 {name}"));
        match r {
            Ok(a) => println!("{name}: ok {}", poppop::format(&a)),
            Err(e) => println!("{name}: ERR {e}"),
        }
    }
}
