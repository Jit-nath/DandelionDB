use std::cmp::Ordering;
use std::collections::HashMap;

use crate::catalog::schema::{Field, Schema};
use crate::dql::ast::{FilterExpr, Operator, ReturnField, Statement, Value, VectorExpr};
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
            Ok(QueryResult::Empty)
        }

        Statement::CreateIndex(stmt) => {
            let collection = db
                .catalog()
                .get_collection(&stmt.collection)
                .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?;
            for field in &stmt.fields {
                if !collection.schema().has_field(field) {
                    return Err(DatabaseError::Validation(format!(
                        "Unknown field '{field}'"
                    )));
                }
            }
            if stmt.index_type != "flat" {
                return Err(DatabaseError::UnsupportedOperation(
                    "only FLAT indexes are implemented",
                ));
            }
            for field in &stmt.fields {
                db.append_index_record(true, &stmt.collection, field)?;
                db.indexes.insert((stmt.collection.clone(), field.clone()));
            }
            Ok(QueryResult::Empty)
        }

        Statement::DropIndex(stmt) => {
            if !db
                .indexes
                .contains(&(stmt.collection.clone(), stmt.field.clone()))
            {
                return Err(DatabaseError::Catalog(format!(
                    "Index not found on '{}.{}'",
                    stmt.collection, stmt.field
                )));
            }
            db.append_index_record(false, &stmt.collection, &stmt.field)?;
            db.indexes
                .remove(&(stmt.collection.clone(), stmt.field.clone()));
            Ok(QueryResult::Empty)
        }

        Statement::Insert(stmt) => insert(db, stmt),

        Statement::Update(stmt) => update(db, stmt),

        Statement::Delete(stmt) => delete(db, stmt),

        Statement::Find(stmt) => find(db, stmt),
    }
}

fn insert(db: &mut Database, stmt: &crate::dql::ast::Insert) -> Result<QueryResult, DatabaseError> {
    let collection = db
        .catalog()
        .get_collection(&stmt.collection)
        .map_err(|e| DatabaseError::Catalog(format!("{e:?}")))?
        .clone();
    let mut values = HashMap::new();
    for field_value in &stmt.values {
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
    let id_field = collection
        .schema()
        .fields()
        .iter()
        .find(|f| f.primary_key && f.auto_increment);
    if let Some(field) = id_field {
        if !values.contains_key(&field.name) {
            values.insert(
                field.name.clone(),
                Value::Number(db.next_row_id.to_string()),
            );
        }
    }
    for field in collection.schema().fields() {
        if !field.nullable
            && !values.contains_key(&field.name)
            && !(field.primary_key && field.auto_increment)
        {
            return Err(DatabaseError::Validation(format!(
                "Missing required field '{}'",
                field.name
            )));
        }
    }
    let row = crate::engine::StoredRow {
        id: db.next_row_id,
        values,
        deleted: false,
    };
    db.next_row_id = db.next_row_id.saturating_add(1);
    db.append_row_insert(&stmt.collection, &row)?;
    db.rows_mut(&stmt.collection).push(row.clone());
    Ok(QueryResult::Affected {
        rows_affected: 1,
        last_insert_id: Some(Value::Number(row.id.to_string())),
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
    validate_value(field, &stmt.value)?;
    let mut changed = Vec::new();
    for row in db.rows(&stmt.collection) {
        if !row.deleted && matches_filter(&stmt.filter, &row.values)? {
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
    Ok(QueryResult::Affected {
        rows_affected: changed.len() as u64,
        last_insert_id: None,
    })
}

fn delete(db: &mut Database, stmt: &crate::dql::ast::Delete) -> Result<QueryResult, DatabaseError> {
    let ids: Vec<u64> = db
        .rows(&stmt.collection)
        .iter()
        .filter(|r| !r.deleted && matches_filter(&stmt.filter, &r.values).unwrap_or(false))
        .map(|r| r.id)
        .collect();
    for id in &ids {
        db.append_row_delete(&stmt.collection, *id)?;
    }
    for row in db.rows_mut(&stmt.collection) {
        if ids.contains(&row.id) {
            row.deleted = true;
        }
    }
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
    let mut ranked = Vec::new();
    for row in db.rows(&stmt.collection).iter().filter(|r| !r.deleted) {
        if let Some(filter) = &stmt.filter {
            if !matches_filter(filter, &row.values)? {
                continue;
            }
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
            if score
                > within
                    .parse::<f64>()
                    .map_err(|_| DatabaseError::Validation("Invalid within value".into()))?
            {
                continue;
            }
        }
        ranked.push((score, row));
    }
    ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
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
                    ReturnField::Field(name) => row
                        .values
                        .get(name)
                        .cloned()
                        .unwrap_or(Value::String("".into())),
                    ReturnField::Score => Value::Number(score.to_string()),
                })
                .collect(),
        })
        .collect();
    Ok(QueryResult::Rows { columns, rows })
}

fn validate_value(field: &Field, value: &Value) -> Result<(), DatabaseError> {
    match (&field.data_type, value) {
        (DataType::Scalar(ScalarType::Text), Value::String(_))
        | (DataType::Scalar(ScalarType::Bytes), Value::String(_))
        | (DataType::Scalar(ScalarType::Boolean), Value::Boolean(_)) => Ok(()),
        (DataType::Scalar(_), Value::Number(value)) => value
            .parse::<f64>()
            .map(|_| ())
            .map_err(|_| DatabaseError::Validation(format!("Invalid value for '{}'", field.name))),
        (DataType::Vector(vector), Value::Vector(values))
            if values.len() == vector.dimension as usize =>
        {
            values.iter().try_for_each(|v| {
                v.parse::<f64>()
                    .map(|_| ())
                    .map_err(|_| DatabaseError::Validation("Invalid vector value".into()))
            })
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
    let dot = dot_product(a, b);
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
    1.0 - a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>()
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
