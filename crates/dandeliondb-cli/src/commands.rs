use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use dandeliondb::{ColumnDefinition, ColumnType, Database, Metric, StoredRecord};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "dandeliondb",
    version,
    about = "Local-first embedded vector database"
)]
struct Cli {
    /// Maximum in-process memory the database may use, in MiB.
    #[arg(long, global = true, default_value_t = 256)]
    memory_budget_mb: usize,
    /// Search worker threads. Defaults to available CPU parallelism.
    #[arg(long, global = true)]
    threads: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an empty database file.
    Init { path: PathBuf },
    /// Open a small interactive inspection shell.
    Shell { path: PathBuf },
    /// List collection names.
    Collections { path: PathBuf },
    /// List table names.
    Tables { path: PathBuf },
    /// Create a fixed-dimension collection.
    CreateCollection {
        path: PathBuf,
        name: String,
        #[arg(long)]
        dimension: usize,
        #[arg(long, default_value = "cosine")]
        metric: String,
        /// Metadata field eligible for equality and $in filtering. Repeat as needed.
        #[arg(long = "filter-field")]
        filter_fields: Vec<String>,
    },
    /// Create a typed vector table. Columns use NAME:TYPE or NAME:TYPE:nullable.
    CreateTable {
        path: PathBuf,
        name: String,
        #[arg(long)]
        dimension: usize,
        #[arg(long, default_value = "cosine")]
        metric: String,
        #[arg(long = "column", value_name = "NAME:TYPE[:nullable]")]
        columns: Vec<String>,
        #[arg(long = "filter-field")]
        filter_fields: Vec<String>,
    },
    /// Remove a collection and all of its records.
    DropCollection { path: PathBuf, name: String },
    /// Remove a table and all of its rows.
    DropTable { path: PathBuf, name: String },
    /// Add or replace one row. Vector and data are JSON values.
    Upsert {
        path: PathBuf,
        collection: String,
        id: String,
        #[arg(long)]
        vector: String,
        #[arg(long, alias = "data", default_value = "{}")]
        metadata: String,
    },
    /// Add or replace records from a JSON array file.
    UpsertJson {
        path: PathBuf,
        collection: String,
        records: PathBuf,
    },
    /// Retrieve one record by ID.
    Get {
        path: PathBuf,
        collection: String,
        id: String,
    },
    /// Mark one record as deleted.
    Delete {
        path: PathBuf,
        collection: String,
        id: String,
    },
    /// Count live records in a collection.
    Count { path: PathBuf, collection: String },
    /// Search for the nearest records. Vector and filter are JSON values.
    Query {
        path: PathBuf,
        collection: String,
        #[arg(long)]
        vector: String,
        #[arg(long, default_value_t = 10)]
        top_k: usize,
        #[arg(long, default_value = "exact")]
        mode: String,
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        ef_search: Option<usize>,
        #[arg(long)]
        include_vector: bool,
    },
    /// Build the Vamana approximate-nearest-neighbor index.
    BuildIndex { path: PathBuf, collection: String },
    /// Remove a collection's approximate index.
    DropIndex { path: PathBuf, collection: String },
    /// Compact tombstones and checkpoint the collection.
    Optimize { path: PathBuf, collection: String },
    /// Force a durable checkpoint.
    Flush { path: PathBuf },
    /// Verify all checksums and immutable storage segments.
    Verify { path: PathBuf },
    /// Print database and collection statistics as JSON.
    Stats { path: PathBuf },
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let memory_budget = cli
        .memory_budget_mb
        .checked_mul(1024 * 1024)
        .ok_or("memory budget is too large")?;

    match cli.command {
        Command::Init { path } => {
            let mut database = Database::create(path, memory_budget, cli.threads)?;
            database.close()?;
        }
        Command::Shell { path } => crate::shell::run(&path, memory_budget, cli.threads)?,
        Command::Collections { path } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&database.collection_names()?)?;
            database.close()?;
        }
        Command::Tables { path } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&database.collection_names()?)?;
            database.close()?;
        }
        Command::CreateCollection {
            path,
            name,
            dimension,
            metric,
            filter_fields,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            let metric = Metric::parse(&metric)?;
            database.create_collection(
                name.clone(),
                dimension,
                metric,
                filter_fields.into_iter().collect::<BTreeSet<_>>(),
            )?;
            print_json(&database.collection_config(&name)?)?;
            database.close()?;
        }
        Command::CreateTable {
            path,
            name,
            dimension,
            metric,
            columns,
            filter_fields,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            database.create_table(
                name.clone(),
                dimension,
                Metric::parse(&metric)?,
                filter_fields.into_iter().collect::<BTreeSet<_>>(),
                parse_columns(columns)?,
            )?;
            print_json(&database.collection_config(&name)?)?;
            database.close()?;
        }
        Command::DropCollection { path, name } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&serde_json::json!({"deleted": database.drop_collection(&name)?}))?;
            database.close()?;
        }
        Command::DropTable { path, name } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&serde_json::json!({"deleted": database.drop_collection(&name)?}))?;
            database.close()?;
        }
        Command::Upsert {
            path,
            collection,
            id,
            vector,
            metadata,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            let record = StoredRecord {
                id,
                vector: parse_vector(&vector)?,
                metadata: dandeliondb::metadata::parse_metadata(&metadata)?,
            };
            let count = database.upsert_many(&collection, vec![record])?;
            print_json(&serde_json::json!({"upserted": count}))?;
            database.close()?;
        }
        Command::UpsertJson {
            path,
            collection,
            records,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            let contents = fs::read_to_string(records)?;
            let records: Vec<StoredRecord> = serde_json::from_str(&contents)?;
            let count = database.upsert_many(&collection, records)?;
            print_json(&serde_json::json!({"upserted": count}))?;
            database.close()?;
        }
        Command::Get {
            path,
            collection,
            id,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&database.get(&collection, &id)?)?;
            database.close()?;
        }
        Command::Delete {
            path,
            collection,
            id,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&serde_json::json!({"deleted": database.delete(&collection, &id)?}))?;
            database.close()?;
        }
        Command::Count { path, collection } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&serde_json::json!({"count": database.count(&collection)?}))?;
            database.close()?;
        }
        Command::Query {
            path,
            collection,
            vector,
            top_k,
            mode,
            filter,
            ef_search,
            include_vector,
        } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            let hits = database.query(
                &collection,
                parse_vector(&vector)?,
                top_k,
                &mode,
                filter.as_deref(),
                ef_search,
                include_vector,
            )?;
            print_json(&hits)?;
            database.close()?;
        }
        Command::BuildIndex { path, collection } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            database.build_index(&collection, "vamana")?;
            database.close()?;
        }
        Command::DropIndex { path, collection } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&serde_json::json!({"deleted": database.drop_index(&collection)?}))?;
            database.close()?;
        }
        Command::Optimize { path, collection } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            database.optimize(&collection)?;
            database.close()?;
        }
        Command::Flush { path } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            database.flush()?;
            database.close()?;
        }
        Command::Verify { path } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            database.verify()?;
            println!("verified");
            database.close()?;
        }
        Command::Stats { path } => {
            let mut database = open(&path, memory_budget, cli.threads)?;
            print_json(&database.stats()?)?;
            database.close()?;
        }
    }
    Ok(())
}

fn open(
    path: &PathBuf,
    memory_budget: usize,
    threads: Option<usize>,
) -> Result<Database, Box<dyn Error>> {
    Ok(Database::open(path, memory_budget, threads)?)
}

fn parse_vector(value: &str) -> Result<Vec<f32>, Box<dyn Error>> {
    Ok(serde_json::from_str(value)?)
}

fn parse_columns(values: Vec<String>) -> Result<Vec<ColumnDefinition>, Box<dyn Error>> {
    values
        .into_iter()
        .map(|value| {
            let mut parts = value.split(':');
            let name = parts
                .next()
                .filter(|value| !value.is_empty())
                .ok_or("column name is required")?;
            let kind = parts.next().ok_or("column type is required")?;
            let nullable = match parts.next() {
                None => false,
                Some("nullable") => true,
                Some(_) => return Err("column suffix must be 'nullable'".into()),
            };
            if parts.next().is_some() {
                return Err("column syntax is NAME:TYPE[:nullable]".into());
            }
            Ok(ColumnDefinition {
                name: name.to_owned(),
                column_type: ColumnType::parse(kind)?,
                nullable,
            })
        })
        .collect()
}

fn print_json(value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
