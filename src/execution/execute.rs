use std::cmp::Ordering;
use std::collections::HashMap;

use crate::catalog::schema::{Field, Schema};
use crate::dql::ast::{
    FieldValue, FilterExpr, Operator, ReturnField, Statement, Value, VectorExpr,
};
use crate::engine::Database;
use crate::errors::DatabaseError;
use crate::result::QueryResult;

use crate::types::{DataType, DistanceMetric, ScalarType, VectorElementType, VectorType};

pub fn execute_statement(
    db: &mut Database,
    statement: &Statement,
) -> Result<QueryResult, DatabaseError> {
    match statement {
        Statement::CreateCollection(stmt) => {
            create_collection(db, stmt)?;
            Ok(QueryResult::Empty)
        }

        Statement::DropCollection(stmt) => {
            if !db.catalog().has_collection(&stmt.name) {
                return Err(DatabaseError::Catalog(format!(
                    "CollectionNotFound({:?})",
                    stmt.name
                )));
            }
            db.append_collection_drop(&stmt.name)?;
            db.catalog_mut()
                .drop_collection(&stmt.name)
                .map_err(|err| DatabaseError::Catalog(format!("{err:?}")))?;
            db.rows.remove(&stmt.name);
            db.forget_live_records(&stmt.name);
            Ok(QueryResult::Empty)
        }

        Statement::CreateIndex(stmt) => {
            let collection = db
                .catalog()
                .get_collection(&stmt.collection)
                .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?;
            for field in &stmt.fields {
                let field_definition = collection
                    .schema()
                    .get_field(field)
                    .ok_or_else(|| DatabaseError::Validation(format!("Unknown field '{field}'")))?;
                if !matches!(field_definition.data_type, DataType::Vector(_)) {
                    return Err(DatabaseError::Validation(format!(
                        "Index field '{field}' must be a vector field"
                    )));
                }
                if db
                    .indexes
                    .contains_key(&(stmt.collection.clone(), field.clone()))
                {
                    return Err(DatabaseError::Validation(format!(
                        "An index already exists on '{}.{}'",
                        stmt.collection, field
                    )));
                }
            }
            for field in &stmt.fields {
                let m = index_property(&stmt.properties, "m").unwrap_or(16);
                let ef_construction =
                    index_property(&stmt.properties, "ef_construction").unwrap_or(200);
                let nlist = index_property(&stmt.properties, "nlist").unwrap_or(16);
                db.append_index_record(
                    true,
                    &stmt.collection,
                    field,
                    &stmt.index_type,
                    m,
                    ef_construction,
                    nlist,
                )?;
                db.rebuild_vector_index_with_params(
                    &stmt.collection,
                    field,
                    &stmt.index_type,
                    m,
                    ef_construction,
                    nlist,
                )?;
            }
            Ok(QueryResult::Empty)
        }

        Statement::DropIndex(stmt) => {
            if !db
                .indexes
                .contains_key(&(stmt.collection.clone(), stmt.field.clone()))
            {
                return Err(DatabaseError::Catalog(format!(
                    "Index not found on '{}.{}'",
                    stmt.collection, stmt.field
                )));
            }
            db.append_index_record(false, &stmt.collection, &stmt.field, "flat", 16, 200, 16)?;
            db.indexes
                .remove(&(stmt.collection.clone(), stmt.field.clone()));
            Ok(QueryResult::Empty)
        }

        Statement::Insert(stmt) => insert(db, stmt),

        Statement::Update(stmt) => update(db, stmt),

        Statement::Delete(stmt) => delete(db, stmt),

        Statement::Find(stmt) => find(db, stmt),
        Statement::Get(stmt) => get(db, stmt),
        Statement::CheckCollection(stmt) => check_collection(db, stmt),
        Statement::CheckDatabase => check_database(db),
        Statement::StatsCollection(stmt) => stats_collection(db, stmt),
        Statement::StatsDatabase => stats_database(db),
        Statement::ShowSegments(stmt) => show_segments(db, stmt),
    }
}

fn insert(db: &mut Database, stmt: &crate::dql::ast::Insert) -> Result<QueryResult, DatabaseError> {
    insert_rows(db, &stmt.collection, &stmt.rows)
}

fn index_property(properties: &[crate::dql::ast::IndexProperty], name: &str) -> Option<usize> {
    properties
        .iter()
        .find(|property| property.name == name)
        .and_then(|property| match &property.value {
            Value::Number(value) => value.parse().ok(),
            _ => None,
        })
}

pub(crate) fn insert_rows(
    db: &mut Database,
    collection_name: &str,
    rows_to_insert: &[Vec<FieldValue>],
) -> Result<QueryResult, DatabaseError> {
    if rows_to_insert.is_empty() {
        return Err(DatabaseError::Validation(
            "Insert must contain at least one row".into(),
        ));
    }

    let collection = db
        .catalog()
        .get_collection(collection_name)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();

    let id_field = collection
        .schema()
        .fields()
        .iter()
        .find(|f| f.primary_key && f.auto_increment);
    let mut next_row_id = db
        .rows(collection_name)
        .iter()
        .map(|row| row.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| DatabaseError::Validation("Row ID space exhausted".into()))?;
    let mut prepared = Vec::with_capacity(rows_to_insert.len());

    for row_values in rows_to_insert {
        let mut values = HashMap::new();
        for field_value in row_values {
            if values.contains_key(&field_value.field) {
                return Err(DatabaseError::Validation(format!(
                    "Duplicate field '{}'",
                    field_value.field
                )));
            }
            let field = collection
                .schema()
                .get_field(&field_value.field)
                .ok_or_else(|| {
                    DatabaseError::Validation(format!("Unknown field '{}'", field_value.field))
                })?;
            validate_value(field, &field_value.value)?;
            values.insert(field_value.field.clone(), field_value.value.clone());
        }

        if let Some(field) = id_field {
            if values.contains_key(&field.name) {
                return Err(DatabaseError::Validation(format!(
                    "auto_increment field '{}' must be omitted",
                    field.name
                )));
            }
            values.insert(field.name.clone(), Value::Number(next_row_id.to_string()));
        }
        for field in collection.schema().fields() {
            if !(field.nullable
                || values.contains_key(&field.name)
                || field.primary_key && field.auto_increment)
            {
                return Err(DatabaseError::Validation(format!(
                    "Missing required field '{}'",
                    field.name
                )));
            }
        }

        prepared.push(crate::engine::StoredRow {
            id: next_row_id,
            values,
            deleted: false,
        });
        next_row_id = next_row_id
            .checked_add(1)
            .ok_or_else(|| DatabaseError::Validation("Row ID space exhausted".into()))?;
    }

    for field in collection
        .schema()
        .fields()
        .iter()
        .filter(|field| field.primary_key)
    {
        for (index, row) in prepared.iter().enumerate() {
            let candidate = row
                .values
                .get(&field.name)
                .expect("primary key is non-nullable");
            if db.rows(collection_name).iter().any(|existing| {
                db.is_live_record(collection_name, existing.id)
                    && existing
                        .values
                        .get(&field.name)
                        .is_some_and(|value| value == candidate)
            }) || prepared[..index].iter().any(|previous| {
                previous
                    .values
                    .get(&field.name)
                    .is_some_and(|value| value == candidate)
            }) {
                return Err(DatabaseError::Validation(format!(
                    "Duplicate primary key '{}'",
                    field.name
                )));
            }
        }
    }

    let last_insert_id = prepared.last().map(|row| Value::Number(row.id.to_string()));
    db.append_row_insert_batch(collection_name, &prepared)?;
    db.next_row_id = db.next_row_id.max(next_row_id);
    db.rows_mut(collection_name).extend(prepared);
    db.rebuild_vector_indexes()?;
    Ok(QueryResult::Affected {
        rows_affected: rows_to_insert.len() as u64,
        last_insert_id,
    })
}

fn update(db: &mut Database, stmt: &crate::dql::ast::Update) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    let field = collection
        .schema()
        .get_field(&stmt.field)
        .ok_or_else(|| DatabaseError::Validation(format!("Unknown field '{}'", stmt.field)))?;
    if field.primary_key {
        return Err(DatabaseError::Validation(
            "Primary-key fields are immutable".into(),
        ));
    }
    validate_value(field, &stmt.value)?;
    validate_filter_fields(&stmt.filter, &collection)?;
    let mut changed = Vec::new();
    for row in db.rows(&stmt.collection) {
        if db.is_live_record(&stmt.collection, row.id) && matches_filter(&stmt.filter, &row.values)?
        {
            changed.push(row.id);
        }
    }
    for id in &changed {
        let old = db
            .rows(&stmt.collection)
            .iter()
            .find(|r| r.id == *id)
            .cloned()
            .unwrap();
        let mut values = old.values;
        values.insert(stmt.field.clone(), stmt.value.clone());
        let row = crate::engine::StoredRow {
            id: *id,
            values,
            deleted: false,
        };
        db.append_row_update(&stmt.collection, &row)?;
        if let Some(existing) = db
            .rows_mut(&stmt.collection)
            .iter_mut()
            .find(|r| r.id == *id)
        {
            *existing = row;
        }
    }
    db.rebuild_vector_indexes()?;
    Ok(QueryResult::Affected {
        rows_affected: changed.len() as u64,
        last_insert_id: None,
    })
}

fn delete(db: &mut Database, stmt: &crate::dql::ast::Delete) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    validate_filter_fields(&stmt.filter, &collection)?;
    let mut ids = Vec::new();
    for row in db.rows(&stmt.collection) {
        if db.is_live_record(&stmt.collection, row.id) && matches_filter(&stmt.filter, &row.values)?
        {
            ids.push(row.id);
        }
    }
    for id in &ids {
        db.append_row_delete(&stmt.collection, *id)?;
    }
    for row in db.rows_mut(&stmt.collection) {
        if ids.contains(&row.id) {
            row.deleted = true;
        }
    }
    db.rebuild_vector_indexes()?;
    Ok(QueryResult::Affected {
        rows_affected: ids.len() as u64,
        last_insert_id: None,
    })
}

fn find(db: &mut Database, stmt: &crate::dql::ast::Find) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    let vector_field = collection
        .schema()
        .get_field(&stmt.column)
        .ok_or_else(|| DatabaseError::Validation(format!("Unknown field '{}'", stmt.column)))?;
    let vector_type = match &vector_field.data_type {
        DataType::Vector(value) => value,
        _ => {
            return Err(DatabaseError::Validation(
                "FIND requires a vector field".into(),
            ));
        }
    };
    if let Some(filter) = &stmt.filter {
        validate_filter_fields(filter, &collection)?;
    }
    let query = match &stmt.vector {
        VectorExpr::Literal(values) => values
            .iter()
            .map(|v| {
                v.parse::<f64>()
                    .map_err(|_| DatabaseError::Validation("Invalid query vector".into()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        VectorExpr::Variable(_) => {
            return Err(DatabaseError::Validation(
                "Vector variables require a bound execution environment".into(),
            ));
        }
    };
    if query.len() != vector_type.dimension as usize {
        return Err(DatabaseError::Validation(
            "Query vector dimension does not match field".into(),
        ));
    }
    let search_budget = stmt
        .search
        .as_ref()
        .map(|search| search.value as usize)
        .unwrap_or((stmt.top.limit as usize).saturating_mul(8).max(64));
    let candidate_ids = db
        .indexes
        .get(&(stmt.collection.clone(), stmt.column.clone()))
        .and_then(|index| index.candidates(&query, stmt.top.limit as usize, search_budget));
    let mut ranked = Vec::new();
    for row in db
        .rows(&stmt.collection)
        .iter()
        .filter(|row| db.is_live_record(&stmt.collection, row.id))
    {
        if let Some(candidate_ids) = &candidate_ids
            && !candidate_ids.contains(&row.id)
        {
            continue;
        }
        if let Some(filter) = &stmt.filter
            && !matches_filter(filter, &row.values)?
        {
            continue;
        }
        let value = match row.values.get(&stmt.column) {
            Some(Value::Vector(v)) => v,
            _ => continue,
        };
        if value.len() != query.len() {
            continue;
        }
        let numbers: Vec<f64> = value.iter().filter_map(|v| v.parse().ok()).collect();
        if numbers.len() != query.len() {
            continue;
        }
        let score = match vector_type.metric {
            DistanceMetric::Cosine => cosine(&query, &numbers),
            DistanceMetric::Euclidean => euclidean(&query, &numbers),
            DistanceMetric::DotProduct => dot_product(&query, &numbers),
        };
        if let Some(within) = &stmt.top.within {
            let threshold = within
                .parse::<f64>()
                .map_err(|_| DatabaseError::Validation("Invalid within value".into()))?;
            let outside = match vector_type.metric {
                DistanceMetric::DotProduct => score < threshold,
                DistanceMetric::Cosine | DistanceMetric::Euclidean => score > threshold,
            };
            if outside {
                continue;
            }
        }
        ranked.push((score, row));
    }
    ranked.sort_by(|a, b| {
        let order = match vector_type.metric {
            DistanceMetric::DotProduct => b.0.partial_cmp(&a.0),
            DistanceMetric::Cosine | DistanceMetric::Euclidean => a.0.partial_cmp(&b.0),
        }
        .unwrap_or(Ordering::Equal);
        order.then_with(|| a.1.id.cmp(&b.1.id))
    });
    ranked.truncate(stmt.top.limit as usize);
    let columns = stmt
        .return_fields
        .iter()
        .map(|field| match field {
            ReturnField::Field(name) => collection
                .schema()
                .get_field(name)
                .map(|f| crate::result::Column {
                    name: name.clone(),
                    data_type: f.data_type.clone(),
                })
                .ok_or_else(|| DatabaseError::Validation(format!("Unknown return field '{name}'"))),
            ReturnField::Score => Ok(crate::result::Column {
                name: "_score".into(),
                data_type: DataType::Scalar(ScalarType::Float64),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rows = ranked
        .into_iter()
        .map(|(score, row)| crate::result::Row {
            values: stmt
                .return_fields
                .iter()
                .map(|field| match field {
                    ReturnField::Field(name) => {
                        row.values.get(name).cloned().unwrap_or(Value::Null)
                    }
                    ReturnField::Score => Value::Number(score.to_string()),
                })
                .collect(),
        })
        .collect();
    Ok(QueryResult::Rows { columns, rows })
}

fn get(db: &mut Database, stmt: &crate::dql::ast::Get) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    if let Some(filter) = &stmt.filter {
        validate_filter_fields(filter, &collection)?;
    }

    let columns = stmt
        .return_fields
        .iter()
        .map(|field| match field {
            ReturnField::Field(name) => collection
                .schema()
                .get_field(name)
                .map(|definition| crate::result::Column {
                    name: name.clone(),
                    data_type: definition.data_type.clone(),
                })
                .ok_or_else(|| DatabaseError::Validation(format!("Unknown return field '{name}'"))),
            ReturnField::Score => Err(DatabaseError::Validation(
                "GET does not support _score".into(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut rows = Vec::new();
    for row in db
        .rows(&stmt.collection)
        .iter()
        .filter(|row| db.is_live_record(&stmt.collection, row.id))
    {
        if let Some(filter) = &stmt.filter
            && !matches_filter(filter, &row.values)?
        {
            continue;
        }
        rows.push(crate::result::Row {
            values: stmt
                .return_fields
                .iter()
                .map(|field| match field {
                    ReturnField::Field(name) => {
                        row.values.get(name).cloned().unwrap_or(Value::Null)
                    }
                    ReturnField::Score => Value::Null,
                })
                .collect(),
        });
    }

    Ok(QueryResult::Rows { columns, rows })
}

fn diagnostic_columns() -> Vec<crate::result::Column> {
    [
        ("scope", ScalarType::Text),
        ("status", ScalarType::Text),
        ("issue", ScalarType::Text),
        ("details", ScalarType::Text),
    ]
    .into_iter()
    .map(|(name, scalar)| crate::result::Column {
        name: name.into(),
        data_type: DataType::Scalar(scalar),
    })
    .collect()
}

fn diagnostic_row(
    scope: &str,
    status: &str,
    issue: &str,
    details: impl Into<String>,
) -> crate::result::Row {
    crate::result::Row {
        values: vec![
            Value::String(scope.into()),
            Value::String(status.into()),
            Value::String(issue.into()),
            Value::String(details.into()),
        ],
    }
}

fn check_collection(
    db: &mut Database,
    stmt: &crate::dql::ast::CheckCollection,
) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    let scope = format!("collection:{}", stmt.collection);
    let mut issues = Vec::new();
    let mut primary_values: Vec<Value> = Vec::new();

    for row in db.rows(&stmt.collection) {
        let live = db.is_live_record(&stmt.collection, row.id);
        if live == row.deleted {
            issues.push(diagnostic_row(
                &scope,
                "error",
                "row_live_state",
                format!("row {} has inconsistent live/deleted state", row.id),
            ));
        }
        if !live {
            continue;
        }
        for field in collection.schema().fields() {
            match row.values.get(&field.name) {
                Some(value) => {
                    if let Err(error) = validate_value(field, value) {
                        issues.push(diagnostic_row(
                            &scope,
                            "error",
                            "field_value",
                            format!("row {} field '{}': {error}", row.id, field.name),
                        ));
                    }
                }
                None if !(field.nullable || field.primary_key && field.auto_increment) => {
                    issues.push(diagnostic_row(
                        &scope,
                        "error",
                        "missing_required_field",
                        format!("row {} is missing '{}'", row.id, field.name),
                    ));
                }
                None => {}
            }
        }
        for field_name in row.values.keys() {
            if !collection.schema().has_field(field_name) {
                issues.push(diagnostic_row(
                    &scope,
                    "error",
                    "unknown_field",
                    format!("row {} contains unknown field '{}'", row.id, field_name),
                ));
            }
        }
        if let Some(primary) = collection
            .schema()
            .fields()
            .iter()
            .find(|field| field.primary_key)
            && let Some(value) = row.values.get(&primary.name)
        {
            if primary_values.iter().any(|existing| existing == value) {
                issues.push(diagnostic_row(
                    &scope,
                    "error",
                    "duplicate_primary_key",
                    format!("duplicate value in '{}'", primary.name),
                ));
            }
            primary_values.push(value.clone());
        }
    }

    let entries = db.segment_entries().to_vec();
    if !entries.iter().any(|entry| {
        entry.data_type() == crate::engine::TABLE_DATA_TYPE
            && entry.table_number == collection.table_number()
    }) {
        issues.push(diagnostic_row(
            &scope,
            "error",
            "missing_table_segment",
            "no table segment belongs to the collection",
        ));
    }
    for field in collection.schema().fields() {
        if matches!(field.data_type, DataType::Vector(_))
            && !entries.iter().any(|entry| {
                entry.data_type() == crate::engine::VECTOR_DATA_TYPE
                    && entry.table_number == collection.table_number()
                    && entry.column_number == field.column_number
            })
        {
            issues.push(diagnostic_row(
                &scope,
                "error",
                "missing_vector_segment",
                format!("no vector segment for '{}'", field.name),
            ));
        }
    }

    if issues.is_empty() {
        issues.push(diagnostic_row(
            &scope,
            "ok",
            "none",
            "collection is consistent",
        ));
    }
    Ok(QueryResult::Rows {
        columns: diagnostic_columns(),
        rows: issues,
    })
}

fn check_database(db: &mut Database) -> Result<QueryResult, DatabaseError> {
    let scope = "database";
    let mut issues = Vec::new();
    let file_size = db.file_size()?;
    let mut entries = db.segment_entries().to_vec();
    entries.sort_unstable_by_key(|entry| entry.offset);
    let mut previous_end = crate::storage::segment_index::HEADER_SIZE + db.segment_index_size();
    for entry in &entries {
        let end = entry.offset.saturating_add(entry.total_size);
        if entry.offset < previous_end || end > file_size {
            issues.push(diagnostic_row(
                scope,
                "error",
                "segment_range",
                format!("segment {} has an invalid physical range", entry.segment_id),
            ));
        }
        if !matches!(
            entry.data_type(),
            crate::engine::SCHEMA_DATA_TYPE
                | crate::engine::TABLE_DATA_TYPE
                | crate::engine::VECTOR_DATA_TYPE
        ) {
            issues.push(diagnostic_row(
                scope,
                "error",
                "segment_type",
                format!(
                    "segment {} has unknown type {}",
                    entry.segment_id,
                    entry.data_type()
                ),
            ));
        }
        previous_end = previous_end.max(end);
    }

    for collection in db.catalog().collections() {
        if !entries.iter().any(|entry| {
            entry.data_type() == crate::engine::TABLE_DATA_TYPE
                && entry.table_number == collection.table_number()
        }) {
            issues.push(diagnostic_row(
                scope,
                "error",
                "catalog_segment_ownership",
                format!("collection '{}' has no table segment", collection.name()),
            ));
        }
    }

    match db.read_data_records_for_check() {
        Ok(records) => {
            let (_, incomplete, transaction_issues) = transaction_summary(&records);
            if incomplete > 0 {
                issues.push(diagnostic_row(
                    scope,
                    "warning",
                    "incomplete_transaction",
                    format!("{incomplete} incomplete transaction(s) are hidden"),
                ));
            }
            for issue in transaction_issues {
                issues.push(diagnostic_row(scope, "error", "transaction", issue));
            }
        }
        Err(error) => issues.push(diagnostic_row(
            scope,
            "error",
            "data_log",
            format!("data log could not be scanned: {error}"),
        )),
    }

    if issues.is_empty() {
        issues.push(diagnostic_row(
            scope,
            "ok",
            "none",
            "database is consistent",
        ));
    }
    Ok(QueryResult::Rows {
        columns: diagnostic_columns(),
        rows: issues,
    })
}

fn transaction_summary(records: &[Vec<u8>]) -> (u64, u64, Vec<String>) {
    let mut pending: Option<(u64, u32, usize)> = None;
    let mut committed = 0;
    let mut issues = Vec::new();
    for record in records {
        match record.first().copied() {
            Some(7) if record.len() == 13 => {
                if pending.is_some() {
                    issues.push("nested transaction begin marker".into());
                }
                let id = u64::from_le_bytes(record[1..9].try_into().unwrap());
                let count = u32::from_le_bytes(record[9..13].try_into().unwrap());
                pending = Some((id, count, 0));
            }
            Some(1) if pending.is_some() => {
                pending.as_mut().unwrap().2 += 1;
            }
            Some(8) if record.len() == 13 => {
                let id = u64::from_le_bytes(record[1..9].try_into().unwrap());
                let count = u32::from_le_bytes(record[9..13].try_into().unwrap());
                match pending.take() {
                    Some((pending_id, expected, actual))
                        if pending_id == id && expected == count && actual == count as usize =>
                    {
                        committed += 1;
                    }
                    Some(_) => issues.push("transaction commit count or ID mismatch".into()),
                    None => issues.push("transaction commit without begin".into()),
                }
            }
            Some(7 | 8) => issues.push("malformed transaction marker".into()),
            Some(_) if pending.is_some() => {
                issues.push("non-row record occurred inside a bulk transaction".into())
            }
            _ => {}
        }
    }
    (committed, u64::from(pending.is_some()), issues)
}

fn stats_collection(
    db: &mut Database,
    stmt: &crate::dql::ast::StatsCollection,
) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    let rows = db.rows(&stmt.collection);
    let live_rows = db.live_record_count(&stmt.collection) as u64;
    let deleted_rows = rows.len() as u64 - live_rows;
    let vector_fields = collection
        .schema()
        .fields()
        .iter()
        .filter(|field| matches!(field.data_type, DataType::Vector(_)))
        .count() as u64;
    let vector_values = rows
        .iter()
        .filter(|row| db.is_live_record(&stmt.collection, row.id))
        .flat_map(|row| {
            collection
                .schema()
                .fields()
                .iter()
                .filter_map(move |field| {
                    matches!(field.data_type, DataType::Vector(_))
                        .then(|| {
                            row.values
                                .get(&field.name)
                                .filter(|value| matches!(value, Value::Vector(_)))
                        })
                        .flatten()
                })
        })
        .count() as u64;
    let entries: Vec<_> = db
        .segment_entries()
        .iter()
        .filter(|entry| entry.table_number == collection.table_number())
        .copied()
        .collect();
    let allocated_bytes = entries.iter().map(|entry| entry.total_size).sum::<u64>();
    let used_bytes = entries
        .iter()
        .filter_map(|entry| db.segment_used_bytes(*entry).ok().flatten())
        .sum::<u64>();
    let indexes = db
        .indexes
        .iter()
        .filter(|((name, _), _)| name == &stmt.collection)
        .count() as u64;
    let columns = stats_columns();
    let row = crate::result::Row {
        values: vec![
            Value::String(collection.name().into()),
            Value::Number(collection.table_number().to_string()),
            Value::Number(live_rows.to_string()),
            Value::Number(deleted_rows.to_string()),
            Value::Number(vector_fields.to_string()),
            Value::Number(vector_values.to_string()),
            Value::Number(indexes.to_string()),
            Value::Number(allocated_bytes.to_string()),
            Value::Number(used_bytes.to_string()),
            Value::String("schema segments only".into()),
        ],
    };
    Ok(QueryResult::Rows {
        columns,
        rows: vec![row],
    })
}

fn stats_columns() -> Vec<crate::result::Column> {
    [
        ("collection", ScalarType::Text),
        ("table_number", ScalarType::UInt64),
        ("live_rows", ScalarType::UInt64),
        ("deleted_rows", ScalarType::UInt64),
        ("vector_fields", ScalarType::UInt64),
        ("vector_values", ScalarType::UInt64),
        ("active_indexes", ScalarType::UInt64),
        ("allocated_bytes", ScalarType::UInt64),
        ("used_bytes", ScalarType::UInt64),
        ("usage_status", ScalarType::Text),
    ]
    .into_iter()
    .map(|(name, scalar)| crate::result::Column {
        name: name.into(),
        data_type: DataType::Scalar(scalar),
    })
    .collect()
}

fn stats_database(db: &mut Database) -> Result<QueryResult, DatabaseError> {
    let mut live_rows = 0u64;
    let mut deleted_rows = 0u64;
    for collection in db.catalog().collections() {
        let rows = db.rows(collection.name());
        live_rows += db.live_record_count(collection.name()) as u64;
        deleted_rows += rows.len() as u64 - db.live_record_count(collection.name()) as u64;
    }
    let entries = db.segment_entries().to_vec();
    let allocated_bytes = entries.iter().map(|entry| entry.total_size).sum::<u64>();
    let mut used_bytes = 0;
    for entry in entries.iter().copied() {
        used_bytes += db.segment_used_bytes(entry)?.unwrap_or(0);
    }
    let records = db.read_data_records_for_check()?;
    let (committed, incomplete, _) = transaction_summary(&records);
    let columns = database_stats_columns();
    let row = crate::result::Row {
        values: vec![
            Value::Number(db.catalog().collections().count().to_string()),
            Value::Number(live_rows.to_string()),
            Value::Number(deleted_rows.to_string()),
            Value::Number(allocated_bytes.to_string()),
            Value::Number(used_bytes.to_string()),
            Value::Number(entries.len().to_string()),
            Value::Number(committed.to_string()),
            Value::Number(incomplete.to_string()),
            Value::Number(db.next_transaction_id().to_string()),
            Value::String("schema segments only".into()),
        ],
    };
    Ok(QueryResult::Rows {
        columns,
        rows: vec![row],
    })
}

fn database_stats_columns() -> Vec<crate::result::Column> {
    [
        ("collections", ScalarType::UInt64),
        ("live_rows", ScalarType::UInt64),
        ("deleted_rows", ScalarType::UInt64),
        ("allocated_bytes", ScalarType::UInt64),
        ("used_bytes", ScalarType::UInt64),
        ("segments", ScalarType::UInt64),
        ("committed_batches", ScalarType::UInt64),
        ("incomplete_batches", ScalarType::UInt64),
        ("next_transaction_id", ScalarType::UInt64),
        ("usage_status", ScalarType::Text),
    ]
    .into_iter()
    .map(|(name, scalar)| crate::result::Column {
        name: name.into(),
        data_type: DataType::Scalar(scalar),
    })
    .collect()
}

fn show_segments(
    db: &mut Database,
    stmt: &crate::dql::ast::ShowSegments,
) -> Result<QueryResult, DatabaseError> {
    let table_number = stmt
        .collection
        .as_ref()
        .map(|name| {
            db.catalog()
                .get_collection(name)
                .map(|collection| collection.table_number())
                .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))
        })
        .transpose()?;
    let mut rows = Vec::new();
    for entry in db.segment_entries().to_vec() {
        if table_number.is_some_and(|number| entry.table_number != number) {
            continue;
        }
        let collection = db
            .catalog()
            .collections()
            .find(|collection| collection.table_number() == entry.table_number)
            .map(|collection| collection.name().to_string())
            .unwrap_or_default();
        let used = db.segment_used_bytes(entry)?;
        rows.push(crate::result::Row {
            values: vec![
                Value::Number(entry.segment_id.to_string()),
                Value::String(segment_type_name(entry.data_type()).into()),
                Value::String(collection),
                Value::Number(entry.table_number.to_string()),
                Value::Number(entry.column_number.to_string()),
                Value::Number(entry.row_start_number.to_string()),
                Value::Number(entry.offset.to_string()),
                Value::Number(entry.total_size.to_string()),
                Value::Number(used.unwrap_or(0).to_string()),
                Value::Number(entry.flags().to_string()),
                Value::String(
                    if used.is_some() {
                        "tracked"
                    } else {
                        "reserved"
                    }
                    .into(),
                ),
            ],
        });
    }
    Ok(QueryResult::Rows {
        columns: segment_columns(),
        rows,
    })
}

fn segment_type_name(data_type: u8) -> &'static str {
    match data_type {
        crate::engine::SCHEMA_DATA_TYPE => "schema",
        crate::engine::TABLE_DATA_TYPE => "table",
        crate::engine::VECTOR_DATA_TYPE => "vector",
        _ => "unknown",
    }
}

fn segment_columns() -> Vec<crate::result::Column> {
    [
        ("segment_id", ScalarType::UInt64),
        ("data_type", ScalarType::Text),
        ("collection", ScalarType::Text),
        ("table_number", ScalarType::UInt64),
        ("column_number", ScalarType::UInt64),
        ("row_start", ScalarType::UInt64),
        ("offset", ScalarType::UInt64),
        ("allocated_bytes", ScalarType::UInt64),
        ("used_bytes", ScalarType::UInt64),
        ("flags", ScalarType::UInt64),
        ("usage_status", ScalarType::Text),
    ]
    .into_iter()
    .map(|(name, scalar)| crate::result::Column {
        name: name.into(),
        data_type: DataType::Scalar(scalar),
    })
    .collect()
}

fn validate_value(field: &Field, value: &Value) -> Result<(), DatabaseError> {
    match (&field.data_type, value) {
        (_, Value::Null) if field.nullable => Ok(()),
        (_, Value::Null) => Err(DatabaseError::Validation(format!(
            "Field '{}' is not nullable",
            field.name
        ))),
        (DataType::Scalar(ScalarType::Text), Value::String(_))
        | (DataType::Scalar(ScalarType::Boolean), Value::Boolean(_)) => Ok(()),
        (DataType::Scalar(ScalarType::Bytes), Value::String(value)) if is_base64_value(value) => {
            Ok(())
        }
        (DataType::Scalar(ScalarType::DateTime), Value::String(value)) if is_rfc3339_utc(value) => {
            Ok(())
        }
        (DataType::Scalar(scalar), Value::Number(value)) => {
            validate_numeric(*scalar, value, &field.name)
        }
        (DataType::Vector(vector), Value::Vector(values))
            if values.len() == vector.dimension as usize =>
        {
            let mut norm_squared = 0.0;
            for value in values {
                validate_vector_element(vector.element_type, value)?;
                let number = value
                    .parse::<f64>()
                    .map_err(|_| DatabaseError::Validation("Invalid vector value".into()))?;
                norm_squared += number * number;
            }
            if matches!(vector.metric, DistanceMetric::Cosine) && norm_squared == 0.0 {
                return Err(DatabaseError::Validation(
                    "Cosine vectors must have non-zero norm".into(),
                ));
            }
            Ok(())
        }
        _ => Err(DatabaseError::Validation(format!(
            "Type mismatch for field '{}'",
            field.name
        ))),
    }
}

fn matches_filter(
    filter: &FilterExpr,
    values: &HashMap<String, Value>,
) -> Result<bool, DatabaseError> {
    match filter {
        FilterExpr::And(a, b) => Ok(matches_filter(a, values)? && matches_filter(b, values)?),
        FilterExpr::Or(a, b) => Ok(matches_filter(a, values)? || matches_filter(b, values)?),
        FilterExpr::Comparison {
            field,
            operator,
            value,
        } => compare(values.get(field), operator, value),
    }
}

fn compare(
    left: Option<&Value>,
    operator: &Operator,
    right: &Value,
) -> Result<bool, DatabaseError> {
    let Some(left) = left else {
        return Ok(false);
    };
    let ordering = match (left, right) {
        (Value::Null, _) | (_, Value::Null) => {
            return Err(DatabaseError::Validation(
                "Null comparisons are not supported".into(),
            ));
        }
        (Value::Number(a), Value::Number(b)) => {
            let a = a
                .parse::<f64>()
                .map_err(|_| DatabaseError::Validation("Invalid numeric filter".into()))?;
            let b = b
                .parse::<f64>()
                .map_err(|_| DatabaseError::Validation("Invalid numeric filter".into()))?;
            a.partial_cmp(&b)
        }
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Boolean(a), Value::Boolean(b)) => Some(a.cmp(b)),
        _ => None,
    }
    .ok_or_else(|| DatabaseError::Validation("Incompatible filter values".into()))?;
    Ok(match operator {
        Operator::Equal => ordering == Ordering::Equal,
        Operator::NotEqual => ordering != Ordering::Equal,
        Operator::Less => ordering == Ordering::Less,
        Operator::Greater => ordering == Ordering::Greater,
        Operator::LessEqual => ordering != Ordering::Greater,
        Operator::GreaterEqual => ordering != Ordering::Less,
    })
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot = raw_dot_product(a, b);
    let na = a.iter().map(|v| v * v).sum::<f64>().sqrt();
    let nb = b.iter().map(|v| v * v).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        1.0
    } else {
        1.0 - dot / (na * nb)
    }
}
fn euclidean(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}
fn dot_product(a: &[f64], b: &[f64]) -> f64 {
    raw_dot_product(a, b)
}

fn raw_dot_product(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn validate_filter_fields(
    filter: &FilterExpr,
    collection: &crate::catalog::Collection,
) -> Result<(), DatabaseError> {
    match filter {
        FilterExpr::Comparison { field, .. } => {
            if collection.schema().has_field(field) {
                Ok(())
            } else {
                Err(DatabaseError::Validation(format!(
                    "Unknown filter field '{field}'"
                )))
            }
        }
        FilterExpr::And(left, right) | FilterExpr::Or(left, right) => {
            validate_filter_fields(left, collection)?;
            validate_filter_fields(right, collection)
        }
    }
}

fn validate_numeric(
    scalar: ScalarType,
    value: &str,
    field_name: &str,
) -> Result<(), DatabaseError> {
    let invalid = || DatabaseError::Validation(format!("Invalid value for '{field_name}'"));
    match scalar {
        ScalarType::UInt8 => value.parse::<u8>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::UInt16 => value.parse::<u16>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::UInt32 => value.parse::<u32>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::UInt64 => value.parse::<u64>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::Int8 => value.parse::<i8>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::Int16 => value.parse::<i16>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::Int32 => value.parse::<i32>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::Int64 => value.parse::<i64>().map(|_| ()).map_err(|_| invalid()),
        ScalarType::Float16 => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && v.abs() <= 65_504.0)
            .map(|_| ())
            .ok_or_else(invalid),
        ScalarType::Float32 => value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|_| ())
            .ok_or_else(invalid),
        ScalarType::Float64 | ScalarType::Float128 => value
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|_| ())
            .ok_or_else(invalid),
        ScalarType::Boolean | ScalarType::Text | ScalarType::Bytes | ScalarType::DateTime => {
            Err(invalid())
        }
    }
}

fn validate_vector_element(
    element_type: VectorElementType,
    value: &str,
) -> Result<(), DatabaseError> {
    let invalid = || DatabaseError::Validation("Invalid vector element value".into());
    match element_type {
        VectorElementType::Binary => match value {
            "0" | "1" => Ok(()),
            _ => Err(invalid()),
        },
        VectorElementType::Int4 => value
            .parse::<i8>()
            .ok()
            .filter(|value| (-8..=7).contains(value))
            .map(|_| ())
            .ok_or_else(invalid),
        VectorElementType::Int8 => value.parse::<i8>().map(|_| ()).map_err(|_| invalid()),
        VectorElementType::Int16 => value.parse::<i16>().map(|_| ()).map_err(|_| invalid()),
        VectorElementType::FP16 => value
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && value.abs() <= 65_504.0)
            .map(|_| ())
            .ok_or_else(invalid),
        VectorElementType::FP32 => value
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| ())
            .ok_or_else(invalid),
        VectorElementType::FP64 => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| ())
            .ok_or_else(invalid),
    }
}

fn is_rfc3339_utc(value: &str) -> bool {
    value.len() >= 20
        && value.ends_with('Z')
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
        && value.as_bytes().get(13) == Some(&b':')
        && value.as_bytes().get(16) == Some(&b':')
}

fn is_base64_value(value: &str) -> bool {
    let Some(encoded) = value.strip_prefix("base64:") else {
        return false;
    };
    !encoded.is_empty()
        && encoded.len() % 4 == 0
        && encoded
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
}

fn create_collection(
    db: &mut Database,
    statement: &crate::dql::ast::CreateCollection,
) -> Result<(), DatabaseError> {
    let mut fields = Vec::with_capacity(statement.fields.len());

    for field in &statement.fields {
        let data_type = parse_data_type(&field.data_type, &field.properties)?;
        let nullable = field.properties.iter().any(|p| p == "nullable");
        let primary_key = field.properties.iter().any(|p| p == "primary");
        let auto_increment = field.properties.iter().any(|p| p == "auto_increment");

        fields.push(Field {
            column_number: u8::try_from(fields.len() + 1).map_err(|_| {
                DatabaseError::Validation("A collection cannot contain more than 255 fields".into())
            })?,
            name: field.name.clone(),
            data_type,
            nullable,
            primary_key,
            auto_increment,
        });
    }

    db.catalog_mut()
        .create_collection(statement.name.clone(), Schema::new(fields))
        .map_err(|err| DatabaseError::Catalog(format!("{err:?}")))?;

    let collection = db
        .catalog()
        .get_collection(&statement.name)
        .map_err(|err| DatabaseError::Catalog(format!("{err:?}")))?
        .clone();
    if let Err(error) = db.persist_collection(&collection) {
        let _ = db.catalog_mut().drop_collection(&statement.name);
        return Err(error);
    }
    Ok(())
}

fn parse_data_type(type_name: &str, properties: &[String]) -> Result<DataType, DatabaseError> {
    let scalar = match type_name {
        "text" => Some(ScalarType::Text),
        "int32" => Some(ScalarType::Int32),
        "int64" => Some(ScalarType::Int64),
        "uint32" => Some(ScalarType::UInt32),
        "uint64" => Some(ScalarType::UInt64),
        "fp16" => Some(ScalarType::Float16),
        "fp32" => Some(ScalarType::Float32),
        "fp64" => Some(ScalarType::Float64),
        "fp128" => Some(ScalarType::Float128),
        "datetime" => Some(ScalarType::DateTime),
        "bool" => Some(ScalarType::Boolean),
        "bytes" => Some(ScalarType::Bytes),
        _ => None,
    };

    if let Some(value) = scalar {
        if properties
            .iter()
            .any(|p| matches!(p.as_str(), "cosine" | "dot_product" | "euclidean"))
        {
            return Err(DatabaseError::Validation(
                "Vector metric requires a vector field".into(),
            ));
        }
        return Ok(DataType::Scalar(value));
    }

    let inner = type_name
        .strip_prefix("vector<")
        .and_then(|v| v.strip_suffix('>'))
        .ok_or_else(|| DatabaseError::Validation(format!("Unknown data type '{type_name}'")))?;
    let (element, dimension) = inner
        .split_once(',')
        .ok_or_else(|| DatabaseError::Validation("Invalid vector type".into()))?;
    let element_type = match element {
        "binary" => VectorElementType::Binary,
        "int4" => VectorElementType::Int4,
        "int8" => VectorElementType::Int8,
        "int16" => VectorElementType::Int16,
        "fp16" => VectorElementType::FP16,
        "fp32" => VectorElementType::FP32,
        "fp64" => VectorElementType::FP64,
        _ => {
            return Err(DatabaseError::Validation(format!(
                "Invalid vector element type '{element}'"
            )));
        }
    };
    let dimension = dimension
        .parse::<u32>()
        .map_err(|_| DatabaseError::Validation("Invalid vector dimension".into()))?;
    let metric_count = properties
        .iter()
        .filter(|p| matches!(p.as_str(), "cosine" | "dot_product" | "euclidean"))
        .count();
    if metric_count != 1 {
        return Err(DatabaseError::Validation(
            "Vector fields must declare exactly one metric".into(),
        ));
    }
    let metric = if properties.iter().any(|p| p == "dot_product") {
        DistanceMetric::DotProduct
    } else if properties.iter().any(|p| p == "euclidean") {
        DistanceMetric::Euclidean
    } else {
        DistanceMetric::Cosine
    };

    Ok(DataType::Vector(VectorType::new(
        element_type,
        dimension,
        metric,
    )))
}
