//! Validated query-language data structures.
//!
//! AST fields are intentionally internal because application code should use
//! [`crate::Database::execute`] rather than construct statements directly.

#[derive(Debug)]
pub enum Statement {
    CreateCollection(CreateCollection),
    DropCollection(DropCollection),

    CreateIndex(CreateIndex),
    DropIndex(DropIndex),

    Insert(Insert),
    Update(Update),
    Delete(Delete),

    Find(Find),
    Get(Get),
    CheckCollection(CheckCollection),
    CheckDatabase,
    StatsCollection(StatsCollection),
    StatsDatabase,
    ShowSegments(ShowSegments),
}

// =========================
// Collection
// =========================

#[derive(Debug)]
pub struct CreateCollection {
    pub(crate) name: String,
    pub(crate) fields: Vec<Field>,
}

#[derive(Debug)]
pub struct Field {
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) properties: Vec<String>,
}

#[derive(Debug)]
pub struct DropCollection {
    pub(crate) name: String,
}

// =========================
// Index
// =========================

#[derive(Debug)]
pub struct CreateIndex {
    pub(crate) collection: String,
    pub(crate) fields: Vec<String>,
    pub(crate) index_type: String,
    pub(crate) properties: Vec<IndexProperty>,
}

#[derive(Debug)]
pub struct IndexProperty {
    pub(crate) name: String,
    pub(crate) value: Value,
}

#[derive(Debug)]
pub struct DropIndex {
    pub(crate) collection: String,
    pub(crate) field: String,
}

// =========================
// Insert
// =========================

#[derive(Debug)]
pub struct Insert {
    pub(crate) collection: String,
    pub(crate) rows: Vec<Vec<FieldValue>>,
}

#[derive(Debug, Clone)]
pub struct FieldValue {
    pub(crate) field: String,
    pub(crate) value: Value,
}

// =========================
// Update
// =========================

#[derive(Debug)]
pub struct Update {
    pub(crate) collection: String,
    pub(crate) field: String,
    pub(crate) value: Value,
    pub(crate) filter: FilterExpr,
}

// =========================
// Delete
// =========================

#[derive(Debug)]
pub struct Delete {
    pub(crate) collection: String,
    pub(crate) filter: FilterExpr,
}

// =========================
// Find
// =========================

#[derive(Debug)]
pub struct Find {
    pub(crate) top: TopClause,
    pub(crate) vector: VectorExpr,
    pub(crate) collection: String,
    pub(crate) column: String,
    pub(crate) filter: Option<FilterExpr>,
    pub(crate) search: Option<SearchClause>,
    pub(crate) return_fields: Vec<ReturnField>,
}

#[derive(Debug)]
pub struct Get {
    pub(crate) collection: String,
    pub(crate) filter: Option<FilterExpr>,
    pub(crate) return_fields: Vec<ReturnField>,
}

#[derive(Debug)]
pub struct CheckCollection {
    pub(crate) collection: String,
}

#[derive(Debug)]
pub struct StatsCollection {
    pub(crate) collection: String,
}

#[derive(Debug)]
pub struct ShowSegments {
    pub(crate) collection: Option<String>,
}

#[derive(Debug)]
pub struct TopClause {
    pub(crate) limit: u64,
    pub(crate) within: Option<String>,
}

#[derive(Debug)]
pub struct SearchClause {
    pub(crate) property: String,
    pub(crate) value: u64,
}

#[derive(Debug)]
pub enum VectorExpr {
    Literal(Vec<f32>),
    Variable(String),
}

#[derive(Debug)]
pub enum ReturnField {
    Field(String),
    Score,
}

// =========================
// Values
// =========================

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    String(String),
    Number(String),
    Boolean(bool),
    Variable(String),
    Vector(crate::types::PackedVector),
}

// =========================
// Filters
// =========================

#[derive(Debug)]
pub enum FilterExpr {
    Comparison {
        field: String,
        operator: Operator,
        value: Value,
    },

    And(Box<FilterExpr>, Box<FilterExpr>),

    Or(Box<FilterExpr>, Box<FilterExpr>),
}

#[derive(Debug)]
pub enum Operator {
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
}
