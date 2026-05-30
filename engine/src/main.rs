use dandeliondb_engine::{Database, Result};

fn main() -> Result<()> {
    let mut db = Database::memory();
    let output = db.execute("SET greeting hello")?;

    println!("{output}");

    Ok(())
}
