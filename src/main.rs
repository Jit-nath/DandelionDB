use dandeliondb::Database;
use dandeliondb::errors::DatabaseError;

fn main() -> Result<(), DatabaseError> {
    let path = "./test.lion";

    let mut db = Database::open(path)?;

    db.execute(
        r#"
    create collection "docs" {
        "doc_id" uint64 primary auto_increment,
        "doc_string" text,
        "doc_embedding" vector<fp32, 5> metric cosine,
    };
    "#,
    )?;

    db.close()?;

    Ok(())
}
