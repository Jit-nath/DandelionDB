#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    UInt8,
    UInt16,
    UInt32,
    UInt64,

    Int8,
    Int16,
    Int32,
    Int64,

    Float32,
    Float64,
    Float16,
    Float128,

    Boolean,

    Text,
    Bytes,
    DateTime,
}
