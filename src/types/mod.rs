pub mod scalar;
pub mod vector;

pub use scalar::ScalarType;
pub use vector::{DistanceMetric, VectorElementType, VectorType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    Scalar(ScalarType),
    Vector(VectorType),
}
