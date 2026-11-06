# Repository Guidelines

## Project Structure & Module Organization

This is a dependency-light Rust 2024 crate for the embedded DandelionDB engine. Runtime code lives in `src/`: `engine.rs` coordinates database lifecycle and persistence; `catalog/` defines collections and schemas; `dql/` tokenizes, parses, and validates queries; `execution/` applies statements; and `storage/` owns the on-disk header, segment index, and inspection helpers. `index/` contains vector-index work. Public crate exports are in `src/lib.rs`; `src/main.rs` is the development executable. Design references are in `docs/` and the product overview is in `README.md`.

## Build, Test, and Development Commands

- `cargo build` compiles the debug library and executable.
- `cargo test` runs unit tests, including persistence round-trip tests in `src/engine.rs`.
- `cargo run -- <path-to-file.lion>` runs the development executable against a Lion database file.
- `cargo fmt --check` verifies Rust formatting; use `cargo fmt` before committing.
- `cargo clippy --all-targets -- -D warnings` performs additional static checks when Clippy is installed.

## Coding Style & Naming Conventions

Follow `rustfmt` defaults: four-space indentation, trailing commas in multiline constructs, and idiomatic Rust ownership/error handling. Use `snake_case` for modules, functions, variables, and test names; `UpperCamelCase` for structs, enums, and traits; and `SCREAMING_SNAKE_CASE` for constants such as file-format magic values. Keep storage-format offsets, lengths, and binary encoding operations explicit and use checked conversions where input or file data is involved.

## Testing Guidelines

Place focused unit tests next to the implementation in a `#[cfg(test)]` module. Name tests by behavior, for example `crud_search_and_index_round_trip`. Persistence changes should create a temporary `.lion` file, exercise close/open recovery, and remove it afterward. Add boundary and corruption tests whenever changing segment, header, record, or decoding logic. Run `cargo test` before opening a pull request.

## Commit & Pull Request Guidelines

Recent history uses short imperative summaries, with occasional `docs:` prefixes. Prefer concise messages such as `fix data-log segment recovery` or `docs: clarify file layout`. Keep commits narrowly scoped. Pull requests should explain the behavior change, note file-format compatibility implications, link the relevant issue when available, and include the commands/tests run. Screenshots are unnecessary unless a documentation or UI change makes them useful.

## Safety & Compatibility

Treat `.lion` files as durable user data. Do not silently change binary layout, magic bytes, or version handling; document compatibility decisions and add reopen tests for every format change.
