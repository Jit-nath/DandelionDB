use std::error::Error;
use std::io::{self, BufRead, Write};
use std::path::Path;

use dandeliondb::Database;

pub fn run(
    path: &Path,
    memory_budget: usize,
    threads: Option<usize>,
) -> Result<(), Box<dyn Error>> {
    let mut database = Database::open(path, memory_budget, threads)?;
    println!("DandelionDB inspection shell. Type 'help' for commands.");

    let stdin = io::stdin();
    let mut output = io::stdout().lock();
    write!(output, "dandeliondb> ")?;
    output.flush()?;
    for line in stdin.lock().lines() {
        let line = line?;
        match line.trim() {
            "" => {}
            "help" => println!("tables | stats | verify | quit"),
            "tables" => println!(
                "{}",
                serde_json::to_string_pretty(&database.collection_names()?)?
            ),
            "stats" => println!("{}", serde_json::to_string_pretty(&database.stats()?)?),
            "verify" => {
                database.verify()?;
                println!("verified");
            }
            "quit" | "exit" => break,
            _ => eprintln!("unknown command; type 'help'"),
        }
        write!(output, "dandeliondb> ")?;
        output.flush()?;
    }
    database.close()?;
    Ok(())
}
