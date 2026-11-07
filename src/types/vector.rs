//! Vector element formats, distance metrics, and vector field metadata.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorElementType {
    Binary,
    Int4,
    Int8,
    Int16,
    FP32,
    FP64,
    FP16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorType {
    pub element_type: VectorElementType,
    pub dimension: u32,
    pub metric: DistanceMetric,
}

/// Compact in-memory representation of a vector value.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedVector {
    values: Box<[f32]>,
}

impl PackedVector {
    pub fn new(values: impl Into<Box<[f32]>>) -> Self {
        Self {
            values: values.into(),
        }
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.values
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl VectorType {
    pub fn new(element_type: VectorElementType, dimension: u32, metric: DistanceMetric) -> Self {
        Self {
            element_type,
            dimension,
            metric,
        }
    }
}
