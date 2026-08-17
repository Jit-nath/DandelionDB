use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

use crc32fast::Hasher;
use memmap2::{Mmap, MmapOptions};
use rayon::prelude::*;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::search;
use crate::{EngineError, Result};

const DEFAULT_GRAPH_DEGREE: usize = 32;
const DEFAULT_CONSTRUCTION_WIDTH: usize = 100;
const VECTOR_SEGMENT_ALIGNMENT: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Metric {
    Cosine,
    Dot,
    Euclidean,
}

impl Metric {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "cosine" => Ok(Self::Cosine),
            "dot" => Ok(Self::Dot),
            "euclidean" => Ok(Self::Euclidean),
            _ => Err(EngineError::InvalidMetric(value.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::Dot => "dot",
            Self::Euclidean => "euclidean",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordMeta {
    pub id: String,
    pub metadata: Map<String, Value>,
    pub deleted: bool,
    pub norm: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
    pub metadata: Map<String, Value>,
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: Map<String, Value>,
}

/// A scalar column stored beside every vector row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Text,
    Integer,
    Float,
}

impl ColumnType {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "integer" | "int" => Ok(Self::Integer),
            "float" | "double" => Ok(Self::Float),
            _ => Err(EngineError::InvalidMetadata),
        }
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Text => value.is_string(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Float => value.as_f64().is_some(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ColumnDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: ColumnType,
    #[serde(default = "default_nullable")]
    pub nullable: bool,
}

fn default_nullable() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct VectorSegment {
    pub offset: u64,
    pub len: u64,
    pub rows: usize,
    pub crc32: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct GraphSegment {
    pub offset: u64,
    pub len: u64,
    pub rows: usize,
    pub degrees_offset: usize,
    pub codes_offset: usize,
    pub scales_offset: usize,
    pub crc32: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GraphIndex {
    pub degree: usize,
    pub construction_width: usize,
    pub entry_point: Option<u32>,
    pub segment: Option<GraphSegment>,
    #[serde(skip)]
    mapped: Option<Mmap>,
    #[serde(skip)]
    pub neighbors: Vec<u32>,
    #[serde(skip)]
    pub degrees: Vec<u16>,
    #[serde(skip)]
    pub codes: Vec<i8>,
    #[serde(skip)]
    pub scales: Vec<f32>,
    #[serde(skip)]
    code_overlays: HashMap<u32, (Vec<i8>, f32)>,
    pub indexed_rows: usize,
    pub dirty: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Collection {
    pub name: String,
    pub dimension: usize,
    pub stride: usize,
    pub metric: Metric,
    #[serde(default)]
    pub columns: BTreeMap<String, ColumnDefinition>,
    pub filter_fields: BTreeSet<String>,
    #[serde(default)]
    filter_index: FilterIndex,
    pub vector_segment: Option<VectorSegment>,
    #[serde(skip)]
    mapped_vectors: Option<Mmap>,
    #[serde(skip)]
    delta_vectors: Vec<f32>,
    #[serde(skip)]
    vector_overlays: HashMap<u32, Vec<f32>>,
    pub records: Vec<RecordMeta>,
    pub id_to_row: HashMap<String, u32>,
    pub graph: Option<GraphIndex>,
    pub mutation_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FilterPredicate {
    Eq(Value),
    In(Vec<Value>),
    GreaterThan(f64),
    GreaterThanOrEqual(f64),
    LessThan(f64),
    LessThanOrEqual(f64),
}

pub type QueryFilter = Vec<(String, FilterPredicate)>;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
enum FilterScalar {
    Null,
    Bool(bool),
    Integer(i64),
    Unsigned(u64),
    Float(u64),
    String(String),
}

type FilterIndex = HashMap<String, HashMap<FilterScalar, RoaringBitmap>>;

#[derive(Clone, Debug)]
struct HeapItem {
    cost: f32,
    row: u32,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost.to_bits() == other.cost.to_bits() && self.row == other.row
    }
}

impl Eq for HeapItem {}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cost
            .total_cmp(&other.cost)
            .then_with(|| self.row.cmp(&other.row))
    }
}

#[derive(Clone, Debug)]
struct MinHeapItem(HeapItem);

impl PartialEq for MinHeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for MinHeapItem {}

impl PartialOrd for MinHeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MinHeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.cmp(&self.0)
    }
}

#[derive(Debug)]
struct TopK {
    limit: usize,
    heap: BinaryHeap<HeapItem>,
}

impl TopK {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            heap: BinaryHeap::with_capacity(limit.saturating_add(1)),
        }
    }

    fn push(&mut self, item: HeapItem) {
        if self.heap.len() < self.limit {
            self.heap.push(item);
        } else if self.heap.peek().is_some_and(|worst| item.cost < worst.cost) {
            self.heap.pop();
            self.heap.push(item);
        }
    }

    fn merge(&mut self, other: Self) {
        for item in other.heap {
            self.push(item);
        }
    }

    fn into_sorted(mut self) -> Vec<HeapItem> {
        let mut result = Vec::with_capacity(self.heap.len());
        while let Some(item) = self.heap.pop() {
            result.push(item);
        }
        result.sort_by(|left, right| {
            left.cost
                .total_cmp(&right.cost)
                .then_with(|| left.row.cmp(&right.row))
        });
        result
    }
}

impl Collection {
    pub fn new(
        name: String,
        dimension: usize,
        metric: Metric,
        filter_fields: BTreeSet<String>,
    ) -> Result<Self> {
        Self::new_with_columns(name, dimension, metric, filter_fields, Vec::new())
    }

    pub fn new_with_columns(
        name: String,
        dimension: usize,
        metric: Metric,
        filter_fields: BTreeSet<String>,
        columns: Vec<ColumnDefinition>,
    ) -> Result<Self> {
        validate_collection_name(&name)?;
        if dimension == 0 {
            return Err(EngineError::InvalidDimension);
        }
        let mut column_map = BTreeMap::new();
        for column in columns {
            validate_collection_name(&column.name)?;
            if column_map.insert(column.name.clone(), column).is_some() {
                return Err(EngineError::InvalidMetadata);
            }
        }
        if !column_map.is_empty()
            && filter_fields
                .iter()
                .any(|field| !column_map.contains_key(field))
        {
            return Err(EngineError::InvalidMetadata);
        }
        let stride = dimension.next_multiple_of(16);
        Ok(Self {
            name,
            dimension,
            stride,
            metric,
            columns: column_map,
            filter_fields,
            filter_index: HashMap::new(),
            vector_segment: None,
            mapped_vectors: None,
            delta_vectors: Vec::new(),
            vector_overlays: HashMap::new(),
            records: Vec::new(),
            id_to_row: HashMap::new(),
            graph: None,
            mutation_count: 0,
        })
    }

    pub fn live_count(&self) -> usize {
        self.id_to_row.len()
    }

    pub fn allocated_bytes(&self) -> usize {
        self.delta_vectors.capacity() * std::mem::size_of::<f32>()
            + self
                .vector_overlays
                .values()
                .map(|vector| vector.capacity() * std::mem::size_of::<f32>())
                .sum::<usize>()
            + filter_index_heap_bytes(&self.filter_index)
            + self.records.capacity() * std::mem::size_of::<RecordMeta>()
            + self
                .records
                .iter()
                .map(|record| record.id.capacity() + json_object_heap_bytes(&record.metadata))
                .sum::<usize>()
            + self.id_to_row.capacity()
                * (std::mem::size_of::<String>() + std::mem::size_of::<u32>() + 8)
            + self.id_to_row.keys().map(String::capacity).sum::<usize>()
            + self.graph.as_ref().map_or(0, |graph| {
                graph.neighbors.capacity() * std::mem::size_of::<u32>()
                    + graph.codes.capacity() * std::mem::size_of::<i8>()
                    + graph.scales.capacity() * std::mem::size_of::<f32>()
                    + graph.degrees.capacity() * std::mem::size_of::<u16>()
                    + graph
                        .code_overlays
                        .values()
                        .map(|(code, _)| code.capacity())
                        .sum::<usize>()
            })
    }

    pub fn upsert(&mut self, record: StoredRecord) -> Result<bool> {
        self.validate_record(&record)?;
        let (vector, vector_norm) = self.prepare_vector(record.vector)?;

        let inserted = if let Some(&row) = self.id_to_row.get(&record.id) {
            let row = row as usize;
            let old_metadata = self.records[row].metadata.clone();
            self.remove_filter_values(row as u32, &old_metadata);
            if row < self.base_rows() {
                self.vector_overlays.insert(row as u32, vector.clone());
            } else {
                let offset = (row - self.base_rows()) * self.stride;
                self.delta_vectors[offset..offset + self.stride].copy_from_slice(&vector);
            }
            self.records[row].metadata = record.metadata;
            self.records[row].norm = vector_norm;
            self.records[row].deleted = false;
            let new_metadata = self.records[row].metadata.clone();
            self.add_filter_values(row as u32, &new_metadata);
            if let Some(graph) = self.graph.as_mut() {
                graph.update_code(row, &vector);
                graph.dirty = true;
            }
            false
        } else {
            let row = u32::try_from(self.records.len()).map_err(|_| EngineError::MemoryBudget {
                required: usize::MAX,
                available: usize::MAX,
            })?;
            self.delta_vectors.extend_from_slice(&vector);
            self.records.push(RecordMeta {
                id: record.id.clone(),
                metadata: record.metadata,
                deleted: false,
                norm: vector_norm,
            });
            self.id_to_row.insert(record.id, row);
            let metadata = self.records[row as usize].metadata.clone();
            self.add_filter_values(row, &metadata);
            if let Some(graph) = self.graph.as_mut() {
                graph.dirty = true;
            }
            true
        };
        self.mutation_count = self.mutation_count.saturating_add(1);
        Ok(inserted)
    }

    pub fn validate_record(&self, record: &StoredRecord) -> Result<()> {
        validate_id(&record.id)?;
        self.prepare_vector(record.vector.clone())?;
        self.validate_metadata(&record.metadata)
    }

    pub fn get(&self, id: &str) -> Option<StoredRecord> {
        let row = *self.id_to_row.get(id)? as usize;
        let record = self.records.get(row)?;
        if record.deleted {
            return None;
        }
        Some(StoredRecord {
            id: record.id.clone(),
            vector: self.vector(row)[..self.dimension].to_vec(),
            metadata: record.metadata.clone(),
        })
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let Some(row) = self.id_to_row.remove(id) else {
            return false;
        };
        self.records[row as usize].deleted = true;
        let metadata = self.records[row as usize].metadata.clone();
        self.remove_filter_values(row, &metadata);
        if let Some(graph) = self.graph.as_mut() {
            graph.dirty = true;
        }
        self.mutation_count = self.mutation_count.saturating_add(1);
        true
    }

    pub fn query_exact(
        &self,
        query: Vec<f32>,
        top_k: usize,
        filter: &QueryFilter,
        include_vector: bool,
    ) -> Result<Vec<SearchHit>> {
        if top_k == 0 {
            return Err(EngineError::InvalidTopK);
        }
        let (query, query_norm) = self.prepare_vector(query)?;
        let allowed = self.filter_bitmap(filter);
        let scan = || {
            (0..self.records.len())
                .into_par_iter()
                .fold(
                    || TopK::new(top_k),
                    |mut top, row| {
                        let record = &self.records[row];
                        if !record.deleted
                            && allowed
                                .as_ref()
                                .is_none_or(|bitmap| bitmap.contains(row as u32))
                            && self.matches_filter(record, filter)
                        {
                            let vector = self.vector(row);
                            top.push(HeapItem {
                                cost: search::cost(
                                    self.metric,
                                    &query,
                                    query_norm,
                                    vector,
                                    record.norm,
                                ),
                                row: row as u32,
                            });
                        }
                        top
                    },
                )
                .reduce(
                    || TopK::new(top_k),
                    |mut left, right| {
                        left.merge(right);
                        left
                    },
                )
        };

        Ok(self.hits_from_items(scan().into_sorted(), include_vector))
    }

    pub fn query_ann(
        &self,
        query: Vec<f32>,
        top_k: usize,
        ef_search: Option<usize>,
        filter: &QueryFilter,
        include_vector: bool,
    ) -> Result<Vec<SearchHit>> {
        if top_k == 0 {
            return Err(EngineError::InvalidTopK);
        }
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| EngineError::IndexMissing(self.name.clone()))?;
        let (query, query_norm) = self.prepare_vector(query)?;
        let ef = ef_search.unwrap_or_else(|| usize::max(64, top_k.saturating_mul(4)));
        let candidate_limit = usize::max(64, top_k.saturating_mul(4));
        let allowed = self.filter_bitmap(filter);
        if let Some(bitmap) = &allowed
            && bitmap.len() as usize <= ef.saturating_mul(4)
        {
            let mut exact = TopK::new(top_k);
            for row in bitmap.iter() {
                let row_index = row as usize;
                let record = &self.records[row_index];
                if record.deleted || !self.matches_filter(record, filter) {
                    continue;
                }
                exact.push(HeapItem {
                    cost: search::cost(
                        self.metric,
                        &query,
                        query_norm,
                        self.vector(row_index),
                        record.norm,
                    ),
                    row,
                });
            }
            return Ok(self.hits_from_items(exact.into_sorted(), include_vector));
        }
        let mut candidates = graph.search(self, &query, query_norm, ef);

        for row in graph.indexed_rows..self.records.len() {
            if !self.records[row].deleted {
                candidates.insert(row as u32);
            }
        }

        let mut exact = TopK::new(usize::max(candidate_limit, top_k));
        for row in candidates {
            let row_index = row as usize;
            let record = &self.records[row_index];
            if record.deleted
                || allowed.as_ref().is_some_and(|bitmap| !bitmap.contains(row))
                || !self.matches_filter(record, filter)
            {
                continue;
            }
            exact.push(HeapItem {
                cost: search::cost(
                    self.metric,
                    &query,
                    query_norm,
                    self.vector(row_index),
                    record.norm,
                ),
                row,
            });
        }
        let mut items = exact.into_sorted();
        items.truncate(top_k);
        Ok(self.hits_from_items(items, include_vector))
    }

    pub fn build_graph(&mut self) -> Result<()> {
        let graph = GraphIndex::build(self, DEFAULT_GRAPH_DEGREE, DEFAULT_CONSTRUCTION_WIDTH);
        self.graph = Some(graph);
        Ok(())
    }

    pub fn drop_graph(&mut self) -> bool {
        self.graph.take().is_some()
    }

    pub fn optimize(&mut self) -> Result<()> {
        if self.records.len() != self.live_count() {
            let mut vectors = Vec::with_capacity(self.live_count() * self.stride);
            let mut records = Vec::with_capacity(self.live_count());
            let mut id_to_row = HashMap::with_capacity(self.live_count());
            for (old_row, record) in self.records.iter().enumerate() {
                if record.deleted {
                    continue;
                }
                let new_row = records.len() as u32;
                vectors.extend_from_slice(self.vector(old_row));
                records.push(record.clone());
                id_to_row.insert(record.id.clone(), new_row);
            }
            self.vector_segment = None;
            self.mapped_vectors = None;
            self.delta_vectors = vectors;
            self.vector_overlays.clear();
            self.records = records;
            self.id_to_row = id_to_row;
            self.rebuild_filter_index();
        }
        if self.graph.is_some() {
            self.build_graph()?;
        }
        Ok(())
    }

    pub fn validate_after_load(&mut self) -> Result<()> {
        if self.dimension == 0 || self.stride < self.dimension || !self.stride.is_multiple_of(16) {
            return Err(EngineError::Corruption(format!(
                "invalid dimensions for collection {}",
                self.name
            )));
        }
        let base_rows = self.base_rows();
        if base_rows > self.records.len()
            || self.delta_vectors.len() != (self.records.len() - base_rows) * self.stride
        {
            return Err(EngineError::Corruption(format!(
                "vector and record lengths differ for collection {}",
                self.name
            )));
        }
        self.id_to_row.clear();
        for (row, record) in self.records.iter().enumerate() {
            if !record.deleted {
                self.id_to_row.insert(record.id.clone(), row as u32);
            }
        }
        self.rebuild_filter_index();
        if let Some(graph) = &self.graph {
            graph.validate(self.records.len(), self.stride)?;
        }
        Ok(())
    }

    fn prepare_vector(&self, values: Vec<f32>) -> Result<(Vec<f32>, f32)> {
        if values.len() != self.dimension {
            return Err(EngineError::DimensionMismatch {
                expected: self.dimension,
                actual: values.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(EngineError::NonFiniteVector);
        }
        let mut vector = vec![0.0; self.stride];
        vector[..self.dimension].copy_from_slice(&values);
        let vector_norm = search::norm(&vector);
        if self.metric == Metric::Cosine && vector_norm == 0.0 {
            return Err(EngineError::ZeroVector);
        }
        Ok((vector, vector_norm))
    }

    fn validate_metadata(&self, metadata: &Map<String, Value>) -> Result<()> {
        if !self.columns.is_empty() {
            for (name, column) in &self.columns {
                match metadata.get(name) {
                    Some(value) if column.column_type.accepts(value) => {}
                    Some(_) => return Err(EngineError::InvalidMetadata),
                    None if !column.nullable => return Err(EngineError::InvalidMetadata),
                    None => {}
                }
            }
            if metadata.keys().any(|name| !self.columns.contains_key(name)) {
                return Err(EngineError::InvalidMetadata);
            }
        }
        for field in &self.filter_fields {
            if let Some(value) = metadata.get(field)
                && !is_filter_scalar(value)
            {
                return Err(EngineError::InvalidMetadata);
            }
        }
        Ok(())
    }

    fn add_filter_values(&mut self, row: u32, metadata: &Map<String, Value>) {
        for field in &self.filter_fields {
            let Some(value) = metadata.get(field).and_then(FilterScalar::from_value) else {
                continue;
            };
            self.filter_index
                .entry(field.clone())
                .or_default()
                .entry(value)
                .or_default()
                .insert(row);
        }
    }

    fn remove_filter_values(&mut self, row: u32, metadata: &Map<String, Value>) {
        for field in &self.filter_fields {
            let Some(value) = metadata.get(field).and_then(FilterScalar::from_value) else {
                continue;
            };
            if let Some(postings) = self.filter_index.get_mut(field)
                && let Some(bitmap) = postings.get_mut(&value)
            {
                bitmap.remove(row);
                if bitmap.is_empty() {
                    postings.remove(&value);
                }
            }
        }
    }

    fn rebuild_filter_index(&mut self) {
        self.filter_index.clear();
        for row in 0..self.records.len() {
            if self.records[row].deleted {
                continue;
            }
            let metadata = self.records[row].metadata.clone();
            self.add_filter_values(row as u32, &metadata);
        }
    }

    fn filter_bitmap(&self, filter: &QueryFilter) -> Option<RoaringBitmap> {
        let mut intersection: Option<RoaringBitmap> = None;
        for (field, predicate) in filter {
            if matches!(
                predicate,
                FilterPredicate::GreaterThan(_)
                    | FilterPredicate::GreaterThanOrEqual(_)
                    | FilterPredicate::LessThan(_)
                    | FilterPredicate::LessThanOrEqual(_)
            ) {
                return None;
            }
            let postings = self.filter_index.get(field);
            let mut predicate_bitmap = RoaringBitmap::new();
            match predicate {
                FilterPredicate::Eq(value) => {
                    if let Some(value) = FilterScalar::from_value(value)
                        && let Some(bitmap) = postings.and_then(|values| values.get(&value))
                    {
                        predicate_bitmap |= bitmap;
                    }
                }
                FilterPredicate::In(values) => {
                    for value in values {
                        if let Some(value) = FilterScalar::from_value(value)
                            && let Some(bitmap) = postings.and_then(|items| items.get(&value))
                        {
                            predicate_bitmap |= bitmap;
                        }
                    }
                }
                FilterPredicate::GreaterThan(_)
                | FilterPredicate::GreaterThanOrEqual(_)
                | FilterPredicate::LessThan(_)
                | FilterPredicate::LessThanOrEqual(_) => return None,
            }
            if let Some(current) = &mut intersection {
                *current &= &predicate_bitmap;
            } else {
                intersection = Some(predicate_bitmap);
            }
        }
        intersection
    }

    fn matches_filter(&self, record: &RecordMeta, filter: &QueryFilter) -> bool {
        filter.iter().all(|(field, predicate)| {
            let value = record.metadata.get(field);
            match predicate {
                FilterPredicate::Eq(expected) => value == Some(expected),
                FilterPredicate::In(expected) => {
                    value.is_some_and(|actual| expected.contains(actual))
                }
                FilterPredicate::GreaterThan(bound) => value
                    .and_then(Value::as_f64)
                    .is_some_and(|actual| actual > *bound),
                FilterPredicate::GreaterThanOrEqual(bound) => value
                    .and_then(Value::as_f64)
                    .is_some_and(|actual| actual >= *bound),
                FilterPredicate::LessThan(bound) => value
                    .and_then(Value::as_f64)
                    .is_some_and(|actual| actual < *bound),
                FilterPredicate::LessThanOrEqual(bound) => value
                    .and_then(Value::as_f64)
                    .is_some_and(|actual| actual <= *bound),
            }
        })
    }

    pub fn vector_bytes(&self) -> usize {
        self.records.len() * self.stride * std::mem::size_of::<f32>()
    }

    pub fn persist_vectors(&mut self, file: &mut File) -> Result<()> {
        if self.records.is_empty() {
            self.vector_segment = None;
            self.mapped_vectors = None;
            self.delta_vectors.clear();
            self.vector_overlays.clear();
            return Ok(());
        }
        let unchanged = self
            .vector_segment
            .is_some_and(|segment| segment.rows == self.records.len())
            && self.delta_vectors.is_empty()
            && self.vector_overlays.is_empty();
        if unchanged {
            return Ok(());
        }

        let end = file.seek(SeekFrom::End(0))?;
        let offset = end.next_multiple_of(VECTOR_SEGMENT_ALIGNMENT);
        if offset > end {
            let padding = vec![0_u8; (offset - end) as usize];
            file.write_all(&padding)?;
        }
        let mut hasher = Hasher::new();
        for row in 0..self.records.len() {
            let bytes = bytemuck::cast_slice(self.vector(row));
            hasher.update(bytes);
            file.write_all(bytes)?;
        }
        let segment = VectorSegment {
            offset,
            len: self.vector_bytes() as u64,
            rows: self.records.len(),
            crc32: hasher.finalize(),
        };
        self.vector_segment = Some(segment);
        self.attach_vector_file(file)?;
        self.delta_vectors.clear();
        self.delta_vectors.shrink_to_fit();
        self.vector_overlays.clear();
        Ok(())
    }

    pub fn attach_vector_file(&mut self, file: &File) -> Result<()> {
        let Some(segment) = self.vector_segment else {
            self.mapped_vectors = None;
            return Ok(());
        };
        let expected = segment
            .rows
            .saturating_mul(self.stride)
            .saturating_mul(std::mem::size_of::<f32>());
        if segment.len != expected as u64 || segment.offset % VECTOR_SEGMENT_ALIGNMENT != 0 {
            return Err(EngineError::Corruption(format!(
                "invalid vector segment for collection {}",
                self.name
            )));
        }
        // SAFETY: published vector segments are immutable and page aligned. The
        // database retains the file for at least as long as the mapping.
        let mapping = unsafe {
            MmapOptions::new()
                .offset(segment.offset)
                .len(segment.len as usize)
                .map(file)?
        };
        self.mapped_vectors = Some(mapping);
        Ok(())
    }

    pub fn persist_graph(&mut self, file: &mut File) -> Result<()> {
        if let Some(graph) = &mut self.graph {
            graph.persist(file, self.stride)?;
        }
        Ok(())
    }

    pub fn attach_graph_file(&mut self, file: &File) -> Result<()> {
        if let Some(graph) = &mut self.graph {
            graph.attach(file, self.stride)?;
        }
        Ok(())
    }

    pub fn verify_storage(&self) -> Result<()> {
        if let Some(segment) = self.vector_segment {
            let mapping = self.mapped_vectors.as_ref().ok_or_else(|| {
                EngineError::Corruption(format!(
                    "vector segment is not mapped for collection {}",
                    self.name
                ))
            })?;
            let mut hasher = Hasher::new();
            hasher.update(mapping.as_ref());
            if hasher.finalize() != segment.crc32 {
                return Err(EngineError::Corruption(format!(
                    "vector checksum failed for collection {}",
                    self.name
                )));
            }
        }
        if let Some(graph) = &self.graph {
            graph.verify()?;
        }
        Ok(())
    }

    fn base_rows(&self) -> usize {
        self.vector_segment.map_or(0, |segment| segment.rows)
    }

    fn vector(&self, row: usize) -> &[f32] {
        if let Some(vector) = self.vector_overlays.get(&(row as u32)) {
            return vector;
        }
        let base_rows = self.base_rows();
        if row < base_rows {
            let mapped = self
                .mapped_vectors
                .as_ref()
                .expect("validated collections with a segment have a mapping");
            let values: &[f32] = bytemuck::cast_slice(mapped.as_ref());
            let offset = row * self.stride;
            return &values[offset..offset + self.stride];
        }
        let offset = (row - base_rows) * self.stride;
        &self.delta_vectors[offset..offset + self.stride]
    }

    fn hits_from_items(&self, items: Vec<HeapItem>, include_vector: bool) -> Vec<SearchHit> {
        items
            .into_iter()
            .map(|item| {
                let record = &self.records[item.row as usize];
                SearchHit {
                    id: record.id.clone(),
                    score: search::score_from_cost(self.metric, item.cost),
                    metadata: record.metadata.clone(),
                    vector: include_vector
                        .then(|| self.vector(item.row as usize)[..self.dimension].to_vec()),
                }
            })
            .collect()
    }
}

impl FilterScalar {
    fn from_value(value: &Value) -> Option<Self> {
        if value.is_null() {
            Some(Self::Null)
        } else if let Some(value) = value.as_bool() {
            Some(Self::Bool(value))
        } else if let Some(value) = value.as_i64() {
            Some(Self::Integer(value))
        } else if let Some(value) = value.as_u64() {
            Some(Self::Unsigned(value))
        } else if let Some(value) = value.as_f64() {
            Some(Self::Float(value.to_bits()))
        } else {
            value.as_str().map(|value| Self::String(value.to_owned()))
        }
    }
}

impl GraphIndex {
    fn build(collection: &Collection, degree: usize, construction_width: usize) -> Self {
        let rows = collection.records.len();
        let mut graph = Self {
            degree,
            construction_width,
            entry_point: None,
            segment: None,
            mapped: None,
            neighbors: vec![u32::MAX; rows.saturating_mul(degree)],
            degrees: vec![0; rows],
            codes: vec![0; rows.saturating_mul(collection.stride)],
            scales: vec![0.0; rows],
            code_overlays: HashMap::new(),
            indexed_rows: rows,
            dirty: false,
        };

        for row in 0..rows {
            graph.update_code(row, collection.vector(row));
            if collection.records[row].deleted {
                continue;
            }
            if graph.entry_point.is_none() {
                graph.entry_point = Some(row as u32);
                continue;
            }
            let query = collection.vector(row);
            let candidates = graph.search_limited(
                collection,
                query,
                collection.records[row].norm,
                construction_width,
                row,
            );
            let mut nearest: Vec<_> = candidates.into_iter().collect();
            nearest.sort_by(|&left, &right| {
                let left_cost = search::cost(
                    collection.metric,
                    query,
                    collection.records[row].norm,
                    collection.vector(left as usize),
                    collection.records[left as usize].norm,
                );
                let right_cost = search::cost(
                    collection.metric,
                    query,
                    collection.records[row].norm,
                    collection.vector(right as usize),
                    collection.records[right as usize].norm,
                );
                left_cost.total_cmp(&right_cost)
            });
            nearest.truncate(degree);
            for neighbor in nearest {
                graph.add_edge(row as u32, neighbor, collection);
                graph.add_edge(neighbor, row as u32, collection);
            }
        }
        graph
    }

    fn validate(&self, rows: usize, stride: usize) -> Result<()> {
        let valid_backing = if let Some(segment) = self.segment {
            segment.rows == self.indexed_rows
                && self.mapped.is_some()
                && segment.offset.is_multiple_of(VECTOR_SEGMENT_ALIGNMENT)
                && segment.len as usize >= segment.scales_offset + segment.rows * 4
        } else {
            self.neighbors.len() == self.indexed_rows.saturating_mul(self.degree)
                && self.degrees.len() == self.indexed_rows
                && self.codes.len() == self.indexed_rows.saturating_mul(stride)
                && self.scales.len() == self.indexed_rows
        };
        if self.degree == 0 || self.indexed_rows > rows || !valid_backing {
            return Err(EngineError::Corruption(
                "invalid graph index layout".to_owned(),
            ));
        }
        Ok(())
    }

    fn update_code(&mut self, row: usize, vector: &[f32]) {
        let max_abs = vector.iter().copied().map(f32::abs).fold(0.0_f32, f32::max);
        let scale = if max_abs == 0.0 { 1.0 } else { max_abs / 127.0 };
        if row >= self.indexed_rows {
            return;
        }
        let mut code = vec![0_i8; vector.len()];
        for (target, &value) in code.iter_mut().zip(vector) {
            *target = quantize(value, scale);
        }
        if self.segment.is_some() {
            self.code_overlays.insert(row as u32, (code, scale));
        } else {
            self.scales[row] = scale;
            let offset = row * vector.len();
            self.codes[offset..offset + vector.len()].copy_from_slice(&code);
        }
    }

    fn search(
        &self,
        collection: &Collection,
        query: &[f32],
        query_norm: f32,
        ef: usize,
    ) -> HashSet<u32> {
        self.search_limited(collection, query, query_norm, ef, self.indexed_rows)
    }

    fn search_limited(
        &self,
        collection: &Collection,
        query: &[f32],
        query_norm: f32,
        ef: usize,
        row_limit: usize,
    ) -> HashSet<u32> {
        let Some(entry) = self
            .entry_point
            .filter(|entry| (*entry as usize) < row_limit)
        else {
            return HashSet::new();
        };
        let mut visited = HashSet::with_capacity(ef.saturating_mul(8));
        let mut frontier = BinaryHeap::new();
        let mut best = TopK::new(ef.max(1));
        let entry_cost = self.quantized_cost(collection, query, query_norm, entry as usize);
        let entry_item = HeapItem {
            cost: entry_cost,
            row: entry,
        };
        frontier.push(MinHeapItem(entry_item.clone()));
        best.push(entry_item);
        visited.insert(entry);

        while let Some(MinHeapItem(current)) = frontier.pop() {
            if best.heap.len() >= ef
                && best
                    .heap
                    .peek()
                    .is_some_and(|worst| current.cost > worst.cost)
            {
                break;
            }
            for &neighbor in self.neighbors_of(current.row) {
                if neighbor == u32::MAX
                    || neighbor as usize >= row_limit
                    || !visited.insert(neighbor)
                {
                    continue;
                }
                let row = neighbor as usize;
                if collection.records[row].deleted {
                    continue;
                }
                let item = HeapItem {
                    cost: self.quantized_cost(collection, query, query_norm, row),
                    row: neighbor,
                };
                let promising = best.heap.len() < ef
                    || best.heap.peek().is_some_and(|worst| item.cost < worst.cost);
                if promising {
                    best.push(item.clone());
                    frontier.push(MinHeapItem(item));
                }
            }
        }
        best.heap.into_iter().map(|item| item.row).collect()
    }

    fn quantized_cost(
        &self,
        collection: &Collection,
        query: &[f32],
        query_norm: f32,
        row: usize,
    ) -> f32 {
        let (code, scale) = self.code_and_scale(row, collection.stride);
        let mut dot = 0.0_f32;
        let mut l2 = 0.0_f32;
        let mut norm_sq = 0.0_f32;
        for (&query_value, &code_value) in query.iter().zip(code) {
            let candidate = code_value as f32 * scale;
            dot = query_value.mul_add(candidate, dot);
            let delta = query_value - candidate;
            l2 = delta.mul_add(delta, l2);
            norm_sq = candidate.mul_add(candidate, norm_sq);
        }
        match collection.metric {
            Metric::Cosine => -(dot / (query_norm * norm_sq.sqrt().max(f32::MIN_POSITIVE))),
            Metric::Dot => -dot,
            Metric::Euclidean => l2,
        }
    }

    fn neighbors_of(&self, row: u32) -> &[u32] {
        let row = row as usize;
        let offset = row * self.degree;
        let degree = self.degree_at(row);
        &self.all_neighbors()[offset..offset + degree]
    }

    pub fn persist(&mut self, file: &mut File, stride: usize) -> Result<()> {
        if self.indexed_rows == 0 {
            self.segment = None;
            self.mapped = None;
            self.neighbors.clear();
            self.degrees.clear();
            self.codes.clear();
            self.scales.clear();
            self.code_overlays.clear();
            return Ok(());
        }
        if self.segment.is_some() && self.code_overlays.is_empty() {
            return Ok(());
        }

        let end = file.seek(SeekFrom::End(0))?;
        let offset = end.next_multiple_of(VECTOR_SEGMENT_ALIGNMENT);
        if offset > end {
            file.write_all(&vec![0_u8; (offset - end) as usize])?;
        }

        let neighbor_bytes = self.indexed_rows * self.degree * std::mem::size_of::<u32>();
        let degrees_offset = neighbor_bytes;
        let codes_offset = degrees_offset + self.indexed_rows * std::mem::size_of::<u16>();
        let scales_offset = (codes_offset + self.indexed_rows * stride).next_multiple_of(4);
        let len = scales_offset + self.indexed_rows * std::mem::size_of::<f32>();
        let mut hasher = Hasher::new();

        let neighbors = bytemuck::cast_slice(self.all_neighbors());
        hasher.update(neighbors);
        file.write_all(neighbors)?;

        for row in 0..self.indexed_rows {
            let value = (self.degree_at(row) as u16).to_le_bytes();
            hasher.update(&value);
            file.write_all(&value)?;
        }

        for row in 0..self.indexed_rows {
            let (code, _) = self.code_and_scale(row, stride);
            let bytes: &[u8] = bytemuck::cast_slice(code);
            hasher.update(bytes);
            file.write_all(bytes)?;
        }
        let padding_len = scales_offset - (codes_offset + self.indexed_rows * stride);
        if padding_len > 0 {
            let padding = vec![0_u8; padding_len];
            hasher.update(&padding);
            file.write_all(&padding)?;
        }
        for row in 0..self.indexed_rows {
            let (_, scale) = self.code_and_scale(row, stride);
            let value = scale.to_le_bytes();
            hasher.update(&value);
            file.write_all(&value)?;
        }

        self.segment = Some(GraphSegment {
            offset,
            len: len as u64,
            rows: self.indexed_rows,
            degrees_offset,
            codes_offset,
            scales_offset,
            crc32: hasher.finalize(),
        });
        self.attach(file, stride)?;
        self.neighbors.clear();
        self.neighbors.shrink_to_fit();
        self.degrees.clear();
        self.degrees.shrink_to_fit();
        self.codes.clear();
        self.codes.shrink_to_fit();
        self.scales.clear();
        self.scales.shrink_to_fit();
        self.code_overlays.clear();
        Ok(())
    }

    pub fn attach(&mut self, file: &File, stride: usize) -> Result<()> {
        let Some(segment) = self.segment else {
            self.mapped = None;
            return Ok(());
        };
        let expected_scales = segment.scales_offset + segment.rows * std::mem::size_of::<f32>();
        if segment.rows != self.indexed_rows
            || segment.offset % VECTOR_SEGMENT_ALIGNMENT != 0
            || segment.degrees_offset != segment.rows * self.degree * std::mem::size_of::<u32>()
            || segment.codes_offset
                != segment.degrees_offset + segment.rows * std::mem::size_of::<u16>()
            || segment.scales_offset < segment.codes_offset + segment.rows * stride
            || segment.len as usize != expected_scales
        {
            return Err(EngineError::Corruption(
                "invalid graph segment layout".to_owned(),
            ));
        }
        // SAFETY: graph segments are immutable after their manifest is
        // published, and the database retains the underlying file.
        self.mapped = Some(unsafe {
            MmapOptions::new()
                .offset(segment.offset)
                .len(segment.len as usize)
                .map(file)?
        });
        Ok(())
    }

    fn verify(&self) -> Result<()> {
        let Some(segment) = self.segment else {
            return Ok(());
        };
        let mapping = self
            .mapped
            .as_ref()
            .ok_or_else(|| EngineError::Corruption("graph segment is not mapped".to_owned()))?;
        let mut hasher = Hasher::new();
        hasher.update(mapping.as_ref());
        if hasher.finalize() != segment.crc32 {
            return Err(EngineError::Corruption(
                "graph segment checksum failed".to_owned(),
            ));
        }
        Ok(())
    }

    fn all_neighbors(&self) -> &[u32] {
        if let (Some(segment), Some(mapped)) = (self.segment, self.mapped.as_ref()) {
            return bytemuck::cast_slice(&mapped[..segment.degrees_offset]);
        }
        &self.neighbors
    }

    fn degree_at(&self, row: usize) -> usize {
        if let (Some(segment), Some(mapped)) = (self.segment, self.mapped.as_ref()) {
            let degrees: &[u16] =
                bytemuck::cast_slice(&mapped[segment.degrees_offset..segment.codes_offset]);
            return degrees[row] as usize;
        }
        self.degrees[row] as usize
    }

    fn code_and_scale(&self, row: usize, stride: usize) -> (&[i8], f32) {
        if let Some((code, scale)) = self.code_overlays.get(&(row as u32)) {
            return (code, *scale);
        }
        if let (Some(segment), Some(mapped)) = (self.segment, self.mapped.as_ref()) {
            let code_offset = segment.codes_offset + row * stride;
            let code: &[i8] = bytemuck::cast_slice(&mapped[code_offset..code_offset + stride]);
            let scales: &[f32] = bytemuck::cast_slice(
                &mapped[segment.scales_offset
                    ..segment.scales_offset + segment.rows * std::mem::size_of::<f32>()],
            );
            return (code, scales[row]);
        }
        let code_offset = row * stride;
        (
            &self.codes[code_offset..code_offset + stride],
            self.scales[row],
        )
    }

    fn add_edge(&mut self, from: u32, to: u32, collection: &Collection) {
        if from == to {
            return;
        }
        let from_index = from as usize;
        let degree = self.degrees[from_index] as usize;
        let offset = from_index * self.degree;
        if self.neighbors[offset..offset + degree].contains(&to) {
            return;
        }
        if degree < self.degree {
            self.neighbors[offset + degree] = to;
            self.degrees[from_index] += 1;
            return;
        }

        let owner = collection.vector(from_index);
        let owner_norm = collection.records[from_index].norm;
        let new_cost = search::cost(
            collection.metric,
            owner,
            owner_norm,
            collection.vector(to as usize),
            collection.records[to as usize].norm,
        );
        let mut worst_slot = None;
        let mut worst_cost = f32::NEG_INFINITY;
        for slot in 0..self.degree {
            let neighbor = self.neighbors[offset + slot] as usize;
            let candidate_cost = search::cost(
                collection.metric,
                owner,
                owner_norm,
                collection.vector(neighbor),
                collection.records[neighbor].norm,
            );
            if candidate_cost > worst_cost {
                worst_cost = candidate_cost;
                worst_slot = Some(slot);
            }
        }
        if new_cost < worst_cost {
            self.neighbors[offset + worst_slot.expect("degree is non-zero")] = to;
        }
    }
}

pub fn validate_collection_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid_first = chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_');
    let valid_rest = chars.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !valid_first || !valid_rest || name.len() > 128 {
        return Err(EngineError::InvalidCollectionName(name.to_owned()));
    }
    Ok(())
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() {
        Err(EngineError::EmptyId)
    } else {
        Ok(())
    }
}

pub fn parse_metadata(value: &str) -> Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(value)?;
    value
        .as_object()
        .cloned()
        .ok_or(EngineError::InvalidMetadata)
}

pub fn parse_filter(value: Option<&str>, allowed_fields: &BTreeSet<String>) -> Result<QueryFilter> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let value: Value = serde_json::from_str(value)
        .map_err(|error| EngineError::InvalidFilter(error.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| EngineError::InvalidFilter("filter must be an object".to_owned()))?;
    let mut filter = Vec::with_capacity(object.len());
    for (field, predicate) in object {
        if !allowed_fields.contains(field) {
            return Err(EngineError::InvalidFilter(format!(
                "field is not indexed: {field}"
            )));
        }
        if let Some(operator) = predicate.as_object() {
            if operator.len() != 1 {
                return Err(EngineError::InvalidFilter(
                    "filters accept exactly one operator".to_owned(),
                ));
            }
            let (operator, operand) = operator.iter().next().expect("operator has one entry");
            let predicate = match operator.as_str() {
                "$in" => {
                    let values = operand.as_array().ok_or_else(|| {
                        EngineError::InvalidFilter("$in expects an array".to_owned())
                    })?;
                    if values.is_empty() || values.iter().any(|value| !is_filter_scalar(value)) {
                        return Err(EngineError::InvalidFilter(
                            "$in expects at least one scalar value".to_owned(),
                        ));
                    }
                    FilterPredicate::In(values.clone())
                }
                "$gt" => FilterPredicate::GreaterThan(filter_number(operand)?),
                "$gte" => FilterPredicate::GreaterThanOrEqual(filter_number(operand)?),
                "$lt" => FilterPredicate::LessThan(filter_number(operand)?),
                "$lte" => FilterPredicate::LessThanOrEqual(filter_number(operand)?),
                _ => {
                    return Err(EngineError::InvalidFilter(
                        "supported operators are $in, $gt, $gte, $lt, and $lte".to_owned(),
                    ));
                }
            };
            filter.push((field.clone(), predicate));
        } else if is_filter_scalar(predicate) {
            filter.push((field.clone(), FilterPredicate::Eq(predicate.clone())));
        } else {
            return Err(EngineError::InvalidFilter(
                "equality filters require scalar values".to_owned(),
            ));
        }
    }
    Ok(filter)
}

fn filter_number(value: &Value) -> Result<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            EngineError::InvalidFilter("range filters require a finite number".to_owned())
        })
}

fn is_filter_scalar(value: &Value) -> bool {
    value.is_null()
        || value.is_boolean()
        || value.is_i64()
        || value.is_u64()
        || value.as_f64().is_some_and(f64::is_finite)
        || value.is_string()
}

fn quantize(value: f32, scale: f32) -> i8 {
    (value / scale).round().clamp(-127.0, 127.0) as i8
}

fn json_object_heap_bytes(object: &Map<String, Value>) -> usize {
    object
        .iter()
        .map(|(key, value)| key.capacity() + json_heap_bytes(value))
        .sum()
}

fn json_heap_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        Value::String(value) => value.capacity(),
        Value::Array(values) => {
            values.capacity() * std::mem::size_of::<Value>()
                + values.iter().map(json_heap_bytes).sum::<usize>()
        }
        Value::Object(object) => {
            object.len() * (std::mem::size_of::<String>() + std::mem::size_of::<Value>())
                + json_object_heap_bytes(object)
        }
    }
}

fn filter_index_heap_bytes(index: &FilterIndex) -> usize {
    index.capacity()
        * (std::mem::size_of::<String>()
            + std::mem::size_of::<HashMap<FilterScalar, RoaringBitmap>>()
            + 8)
        + index
            .iter()
            .map(|(field, postings)| {
                field.capacity()
                    + postings.capacity()
                        * (std::mem::size_of::<FilterScalar>()
                            + std::mem::size_of::<RoaringBitmap>()
                            + 8)
                    + postings
                        .iter()
                        .map(|(value, bitmap)| {
                            let value_bytes = match value {
                                FilterScalar::String(value) => value.capacity(),
                                _ => 0,
                            };
                            value_bytes + bitmap.serialized_size()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Map, Value, json};

    use super::{Collection, ColumnDefinition, ColumnType, Metric, StoredRecord, parse_filter};

    fn record(id: &str, vector: Vec<f32>, metadata: Value) -> StoredRecord {
        StoredRecord {
            id: id.to_owned(),
            vector,
            metadata: metadata.as_object().cloned().unwrap_or_else(Map::new),
        }
    }

    #[test]
    fn exact_search_and_filters_are_deterministic() {
        let mut collection = Collection::new(
            "docs".to_owned(),
            3,
            Metric::Cosine,
            BTreeSet::from(["kind".to_owned()]),
        )
        .unwrap();
        collection
            .upsert(record("a", vec![1.0, 0.0, 0.0], json!({"kind": "x"})))
            .unwrap();
        collection
            .upsert(record("b", vec![0.5, 0.5, 0.0], json!({"kind": "y"})))
            .unwrap();
        let filter = parse_filter(Some(r#"{"kind":"x"}"#), &collection.filter_fields).unwrap();
        let hits = collection
            .query_exact(vec![1.0, 0.0, 0.0], 10, &filter, false)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn graph_search_finds_nearest_vector() {
        let mut collection =
            Collection::new("docs".to_owned(), 2, Metric::Euclidean, BTreeSet::new()).unwrap();
        for index in 0..100 {
            collection
                .upsert(record(
                    &index.to_string(),
                    vec![index as f32, 0.0],
                    json!({}),
                ))
                .unwrap();
        }
        collection.build_graph().unwrap();
        let hits = collection
            .query_ann(vec![42.1, 0.0], 3, None, &Vec::new(), false)
            .unwrap();
        assert_eq!(hits[0].id, "42");
    }

    #[test]
    fn typed_columns_validate_rows_and_support_range_filters() {
        let columns = vec![
            ColumnDefinition {
                name: "text".to_owned(),
                column_type: ColumnType::Text,
                nullable: false,
            },
            ColumnDefinition {
                name: "text_length".to_owned(),
                column_type: ColumnType::Integer,
                nullable: false,
            },
            ColumnDefinition {
                name: "confidence".to_owned(),
                column_type: ColumnType::Float,
                nullable: true,
            },
        ];
        let mut table = Collection::new_with_columns(
            "docs".to_owned(),
            2,
            Metric::Dot,
            BTreeSet::from(["text_length".to_owned(), "confidence".to_owned()]),
            columns,
        )
        .unwrap();
        table
            .upsert(record(
                "short",
                vec![1.0, 0.0],
                json!({"text":"hello", "text_length":5, "confidence":0.95}),
            ))
            .unwrap();
        assert!(
            table
                .upsert(record(
                    "wrong-type",
                    vec![0.0, 1.0],
                    json!({"text":42, "text_length":5}),
                ))
                .is_err()
        );

        let filter = parse_filter(
            Some(r#"{"text_length":{"$lt":10},"confidence":{"$gte":0.9}}"#),
            &table.filter_fields,
        )
        .unwrap();
        let results = table
            .query_exact(vec![1.0, 0.0], 10, &filter, false)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "short");
    }
}
