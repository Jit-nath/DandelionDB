# CLI reference

Every command accepts `--memory-budget-mb <MiB>` and `--threads <count>` before
its subcommand. The default memory budget is 256 MiB.

## Database and table commands

```text
dandeliondb init <path.lion>
dandeliondb tables <path.lion>
dandeliondb create-table <path.lion> <table> --dimension <n> [--metric cosine|dot|euclidean] [--column NAME:TYPE[:nullable]]... [--filter-field NAME]...
dandeliondb drop-table <path.lion> <table>
```

## Row commands

```text
dandeliondb upsert <path.lion> <table> <id> --vector <json-array> --data <json-object>
dandeliondb upsert-json <path.lion> <table> <records.json>
dandeliondb get <path.lion> <table> <id>
dandeliondb delete <path.lion> <table> <id>
dandeliondb count <path.lion> <table>
```

`upsert` replaces an existing ID. `get`, `query`, `tables`, `stats`, and other
data-returning commands print JSON.

## Search and index commands

```text
dandeliondb query <path.lion> <table> --vector <json-array> [--top-k 10] [--mode exact|ann|auto] [--filter <json-object>] [--ef-search <n>] [--include-vector]
dandeliondb build-index <path.lion> <table>
dandeliondb drop-index <path.lion> <table>
dandeliondb optimize <path.lion> <table>
```

## Maintenance commands

```text
dandeliondb flush <path.lion>
dandeliondb verify <path.lion>
dandeliondb stats <path.lion>
dandeliondb shell <path.lion>
```

`verify` scans immutable vector and graph segments and checks their checksums.
The current inspection shell supports `tables`, `stats`, `verify`, `help`, and
`quit`.

The older `collection` command names remain as compatibility aliases in this
development release. New scripts should use the table commands.
