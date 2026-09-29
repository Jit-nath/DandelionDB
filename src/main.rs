use dandeliondb::Database;
use dandeliondb::errors::DatabaseError;
use std::time::Instant;

fn main() -> Result<(), DatabaseError> {
    let path = "./test.lion";

    let mut db = Database::create(path)?;

    let started_at = Instant::now();
    db.execute(
        r#"
    create collection "company_docs" {
        "doc_id" uint64 primary auto_increment,
        "doc_string" text,
        "doc_embedding" vector<fp32, 768> metric cosine,
        "date_created" datetime,
    };
    "#,
    )?;

    println!("CREATE COLLECTION took {:?}", started_at.elapsed());

    db.close()?;

    Ok(())
}
