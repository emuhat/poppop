use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

struct Entry {
    #[allow(dead_code)]
    input: String,
    #[allow(dead_code)]
    answer: Result<poppop::Answer, poppop::Error>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = poppop::Engine::new();
    let mut history: Vec<Entry> = Vec::new();
    let mut rl = DefaultEditor::new()?;

    loop {
        match rl.readline("> ") {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                rl.add_history_entry(&line).ok();
                let answer = engine.eval(&line);
                match &answer {
                    Ok(a) => println!("{}", poppop::format(a)),
                    Err(e) => println!("error: {e}"),
                }
                history.push(Entry { input: line, answer });
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }
    Ok(())
}
