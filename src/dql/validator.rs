//! Semantic validation for parsed DQL statements.
//!
//! This is the last stage before execution. Keep checks here when they can be
//! decided from the statement alone; catalog-dependent checks belong in the
//! execution layer.

use crate::dql::ast::*;

pub fn validate(statement: &Statement) -> Result<(), String> {
    match statement {
        Statement::CreateCollection(stmt) => validate_create_collection(stmt),
        Statement::DropCollection(stmt) => validate_drop_collection(stmt),
        Statement::CreateIndex(stmt) => validate_create_index(stmt),
        Statement::DropIndex(stmt) => validate_drop_index(stmt),
        Statement::Insert(stmt) => validate_insert(stmt),
        Statement::Update(stmt) => validate_update(stmt),
        Statement::Delete(stmt) => validate_delete(stmt),
        Statement::Find(stmt) => validate_find(stmt),
        Statement::Get(stmt) => validate_get(stmt),
        Statement::CheckCollection(stmt) => validate_named_collection(&stmt.collection),
        Statement::CheckDatabase | Statement::StatsDatabase => Ok(()),
        Statement::StatsCollection(stmt) => validate_named_collection(&stmt.collection),
        Statement::ShowSegments(stmt) => stmt
            .collection
            .as_ref()
            .map_or(Ok(()), |name| validate_named_collection(name)),
    }
}

fn validate_create_collection(stmt: &CreateCollection) -> Result<(), String> {
    if stmt.name.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    if stmt.fields.is_empty() {
        return Err("Collection must contain at least one field".to_string());
    }

    for field in &stmt.fields {
        validate_field(field)?;
    }

    for (index, field) in stmt.fields.iter().enumerate() {
        if stmt.fields[..index]
            .iter()
            .any(|other| other.name == field.name)
        {
            return Err(format!("Duplicate field '{}'", field.name));
        }
    }

    let primary_fields: Vec<_> = stmt
        .fields
        .iter()
        .filter(|field| {
            field
                .properties
                .iter()
                .any(|property| property == "primary")
        })
        .collect();
    if primary_fields.len() > 1 {
        return Err("A collection can declare at most one primary key".to_string());
    }
    for field in &stmt.fields {
        let has = |property: &str| field.properties.iter().any(|value| value == property);
        if has("primary") && has("nullable") {
            return Err("A primary key cannot be nullable".to_string());
        }
        if has("auto_increment") && !matches!(field.data_type.as_str(), "uint32" | "uint64") {
            return Err("auto_increment requires a uint32 or uint64 field".to_string());
        }
    }

    Ok(())
}

fn validate_field(field: &Field) -> Result<(), String> {
    if field.name.is_empty() {
        return Err("Field name cannot be empty".to_string());
    }

    if field.name.starts_with('_') {
        return Err(format!("Field name '{}' is reserved", field.name));
    }

    validate_data_type(&field.data_type)?;
    validate_field_properties(&field.properties)?;

    let metric_count = field
        .properties
        .iter()
        .filter(|property| matches!(property.as_str(), "cosine" | "dot_product" | "euclidean"))
        .count();
    let is_vector = field.data_type.starts_with("vector<");
    if is_vector && metric_count != 1 {
        return Err("Vector fields must declare exactly one metric".to_string());
    }
    if !is_vector && metric_count != 0 {
        return Err("Vector metric requires a vector field".to_string());
    }

    Ok(())
}

fn validate_data_type(data_type: &str) -> Result<(), String> {
    let scalar_types = [
        "text", "int32", "int64", "uint32", "uint64", "fp16", "fp32", "fp64", "fp128", "datetime",
        "bool", "bytes",
    ];

    if scalar_types.contains(&data_type) {
        return Ok(());
    }

    if data_type.starts_with("vector<") && data_type.ends_with('>') {
        validate_vector_type(data_type)?;
        return Ok(());
    }

    Err(format!("Unknown data type '{}'", data_type))
}

fn validate_vector_type(data_type: &str) -> Result<(), String> {
    let inner = data_type
        .strip_prefix("vector<")
        .and_then(|v| v.strip_suffix('>'))
        .ok_or("Invalid vector type")?;

    let mut parts = inner.split(',');

    let element_type = parts.next().ok_or("Missing vector element type")?;

    let dimension = parts.next().ok_or("Missing vector dimension")?;

    if parts.next().is_some() {
        return Err("Invalid vector type".to_string());
    }

    let element_types = ["binary", "int4", "int8", "int16", "fp16", "fp32", "fp64"];

    if !element_types.contains(&element_type) {
        return Err(format!("Invalid vector element type '{}'", element_type));
    }

    let dimension = dimension
        .parse::<u64>()
        .map_err(|_| "Invalid vector dimension".to_string())?;

    if dimension == 0 {
        return Err("Vector dimension must be greater than zero".to_string());
    }

    Ok(())
}

fn validate_field_properties(properties: &[String]) -> Result<(), String> {
    let metrics = properties
        .iter()
        .filter(|property| matches!(property.as_str(), "cosine" | "dot_product" | "euclidean"))
        .count();

    for property in properties {
        match property.as_str() {
            "primary" | "auto_increment" | "nullable" => {}

            "metric" => {
                // Metric requires an associated value.
                // Your current AST stores these as:
                // ["metric", "cosine"]
                //
                // Handle this below.
            }

            "cosine" | "dot_product" | "euclidean" => {}

            _ => {
                return Err(format!("Unknown field property '{}'", property));
            }
        }
    }

    if metrics > 1 {
        return Err("A field can declare only one vector metric".to_string());
    }

    if properties.contains(&"auto_increment".to_string())
        && !properties.contains(&"primary".to_string())
    {
        return Err("auto_increment requires primary".to_string());
    }

    Ok(())
}

fn validate_create_index(stmt: &CreateIndex) -> Result<(), String> {
    if stmt.collection.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    if stmt.fields.is_empty() {
        return Err("Index must contain at least one field".to_string());
    }

    match stmt.index_type.as_str() {
        "flat" => {
            if !stmt.properties.is_empty() {
                return Err("Flat index does not support index properties".to_string());
            }
        }

        "hnsw" => {
            validate_hnsw_properties(&stmt.properties)?;
        }

        "ivf" => {
            validate_ivf_properties(&stmt.properties)?;
        }

        _ => {
            return Err(format!("Unknown index type '{}'", stmt.index_type));
        }
    }

    Ok(())
}

fn validate_hnsw_properties(properties: &[IndexProperty]) -> Result<(), String> {
    for property in properties {
        match property.name.as_str() {
            "m" | "ef_construction" => {
                validate_positive_integer(&property.value)?;
            }

            _ => {
                return Err(format!("Unknown HNSW property '{}'", property.name));
            }
        }
    }

    Ok(())
}

fn validate_ivf_properties(properties: &[IndexProperty]) -> Result<(), String> {
    for property in properties {
        match property.name.as_str() {
            "nlist" => {
                validate_positive_integer(&property.value)?;
            }

            _ => {
                return Err(format!("Unknown IVF property '{}'", property.name));
            }
        }
    }

    Ok(())
}

fn validate_positive_integer(value: &Value) -> Result<(), String> {
    match value {
        Value::Number(value) => {
            let number = value
                .parse::<u64>()
                .map_err(|_| "Expected positive integer".to_string())?;

            if number == 0 {
                return Err("Value must be greater than zero".to_string());
            }

            Ok(())
        }

        _ => Err("Expected integer value".to_string()),
    }
}

fn validate_drop_collection(stmt: &DropCollection) -> Result<(), String> {
    if stmt.name.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    Ok(())
}

fn validate_drop_index(stmt: &DropIndex) -> Result<(), String> {
    if stmt.collection.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    if stmt.field.is_empty() {
        return Err("Field name cannot be empty".to_string());
    }

    Ok(())
}

fn validate_insert(stmt: &Insert) -> Result<(), String> {
    if stmt.collection.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    if stmt.rows.is_empty() {
        return Err("Insert must contain at least one row".to_string());
    }

    for row in &stmt.rows {
        if row.is_empty() {
            return Err("Insert row cannot be empty".to_string());
        }
        for value in row {
            if value.field.is_empty() {
                return Err("Field name cannot be empty".to_string());
            }
            validate_value(&value.value)?;
        }
    }

    Ok(())
}

fn validate_update(stmt: &Update) -> Result<(), String> {
    if stmt.collection.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    if stmt.field.is_empty() {
        return Err("Field name cannot be empty".to_string());
    }

    validate_filter(&stmt.filter)?;

    Ok(())
}

fn validate_delete(stmt: &Delete) -> Result<(), String> {
    if stmt.collection.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }

    validate_filter(&stmt.filter)?;

    Ok(())
}

fn validate_find(stmt: &Find) -> Result<(), String> {
    if stmt.top.limit == 0 {
        return Err("'top' must be greater than zero".to_string());
    }

    if let Some(within) = &stmt.top.within {
        let value = within
            .parse::<f64>()
            .map_err(|_| "Invalid within value".to_string())?;

        if value < 0.0 {
            return Err("'within' cannot be negative".to_string());
        }
    }

    if let Some(filter) = &stmt.filter {
        validate_filter(filter)?;
    }

    if let Some(search) = &stmt.search {
        if search.property != "ef" && search.property != "nprobe" {
            return Err(format!("Unknown search property '{}'", search.property));
        }
        if search.value == 0 {
            return Err("Search value must be greater than zero".to_string());
        }
    }

    if stmt.return_fields.is_empty() {
        return Err("Find must return at least one field".to_string());
    }

    match &stmt.vector {
        VectorExpr::Literal(values) if values.is_empty() => {
            return Err("Query vector cannot be empty".to_string());
        }
        VectorExpr::Literal(values) => {
            for value in values {
                let number = value
                    .parse::<f64>()
                    .map_err(|_| "Invalid query vector value".to_string())?;
                if !number.is_finite() {
                    return Err("Query vector values must be finite".to_string());
                }
            }
        }
        VectorExpr::Variable(name) if name.is_empty() => {
            return Err("Variable name cannot be empty".to_string());
        }
        VectorExpr::Variable(_) => {}
    }

    Ok(())
}

fn validate_get(stmt: &Get) -> Result<(), String> {
    validate_named_collection(&stmt.collection)?;
    if stmt.return_fields.is_empty() {
        return Err("GET must return at least one field".to_string());
    }
    if stmt
        .return_fields
        .iter()
        .any(|field| matches!(field, ReturnField::Score))
    {
        return Err("GET does not support _score".to_string());
    }
    if let Some(filter) = &stmt.filter {
        validate_filter(filter)?;
    }
    Ok(())
}

fn validate_named_collection(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Collection name cannot be empty".to_string());
    }
    Ok(())
}

fn validate_filter(filter: &FilterExpr) -> Result<(), String> {
    match filter {
        FilterExpr::Comparison {
            field,
            operator: _,
            value,
        } => {
            if field.is_empty() {
                return Err("Filter field cannot be empty".to_string());
            }

            validate_value(value)?;

            Ok(())
        }

        FilterExpr::And(left, right) => {
            validate_filter(left)?;
            validate_filter(right)?;
            Ok(())
        }

        FilterExpr::Or(left, right) => {
            validate_filter(left)?;
            validate_filter(right)?;
            Ok(())
        }
    }
}

fn validate_value(value: &Value) -> Result<(), String> {
    match value {
        Value::Null => Ok(()),
        Value::String(_) => Ok(()),
        Value::Number(number) => {
            number
                .parse::<f64>()
                .map_err(|_| "Invalid numeric value".to_string())?;

            Ok(())
        }
        Value::Boolean(_) => Ok(()),
        Value::Variable(name) => {
            if name.is_empty() {
                return Err("Variable name cannot be empty".to_string());
            }

            Ok(())
        }
        Value::Vector(values) => {
            if values.is_empty() {
                return Err("Vector cannot be empty".to_string());
            }

            for value in values {
                value
                    .parse::<f64>()
                    .map_err(|_| "Invalid vector value".to_string())?;
            }

            Ok(())
        }
    }
}
