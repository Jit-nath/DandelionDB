use std::process::{Command, Output};

use tempfile::tempdir;

fn dandelion(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dandeliondb"))
        .args(arguments)
        .output()
        .expect("CLI should start")
}

fn assert_success(output: Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI output must be UTF-8")
}

#[test]
fn cli_persists_and_queries_a_database() {
    let directory = tempdir().expect("temporary directory should exist");
    let database = directory.path().join("cli.lion");
    let path = database.to_str().expect("temporary path must be UTF-8");

    assert_success(dandelion(&["init", path]));
    assert_success(dandelion(&[
        "create-collection",
        path,
        "docs",
        "--dimension",
        "3",
        "--filter-field",
        "kind",
    ]));
    assert_success(dandelion(&[
        "upsert",
        path,
        "docs",
        "intro",
        "--vector",
        "[1.0,0.0,0.0]",
        "--metadata",
        r#"{"kind":"guide"}"#,
    ]));

    let results = assert_success(dandelion(&[
        "query",
        path,
        "docs",
        "--vector",
        "[1.0,0.0,0.0]",
        "--filter",
        r#"{"kind":"guide"}"#,
    ]));
    assert!(results.contains(r#""id": "intro""#));
    assert_success(dandelion(&["verify", path]));
}

#[test]
fn cli_creates_typed_tables_and_filters_numeric_columns() {
    let directory = tempdir().expect("temporary directory should exist");
    let database = directory.path().join("typed.lion");
    let path = database.to_str().expect("temporary path must be UTF-8");

    assert_success(dandelion(&["init", path]));
    assert_success(dandelion(&[
        "create-table",
        path,
        "documents",
        "--dimension",
        "2",
        "--column",
        "text:text",
        "--column",
        "text_length:integer",
        "--column",
        "confidence:float:nullable",
        "--filter-field",
        "text_length",
        "--filter-field",
        "confidence",
    ]));
    assert_success(dandelion(&[
        "upsert",
        path,
        "documents",
        "doc-1",
        "--vector",
        "[1.0,0.0]",
        "--data",
        r#"{"text":"hello","text_length":5,"confidence":0.95}"#,
    ]));
    let results = assert_success(dandelion(&[
        "query",
        path,
        "documents",
        "--vector",
        "[1.0,0.0]",
        "--filter",
        r#"{"confidence":{"$gte":0.9}}"#,
    ]));
    assert!(results.contains(r#""id": "doc-1""#));
}
