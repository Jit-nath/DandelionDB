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

impl VectorType {
    pub fn new(element_type: VectorElementType, dimension: u32, metric: DistanceMetric) -> Self {
        Self {
            element_type,
            dimension,
            metric,
        }
    }
}
