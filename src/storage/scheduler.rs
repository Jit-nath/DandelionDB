//! Capacity planning for on-disk database segments.
//!
//! Reservations are always power-of-two megabyte segments.  This makes growth
//! predictable and prevents a small schema or vector from creating thousands
//! of tiny extents in the database file.

use crate::catalog::Schema;
use crate::types::{DataType, ScalarType, VectorElementType};

pub const MIB: u64 = 1024 * 1024;
pub const MIN_RESERVATION_BYTES: u64 = MIB;
pub const MAX_RESERVATION_BYTES: u64 = 256 * MIB;
pub const DEFAULT_RECORD_FORECAST: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationKind {
    Table,
    Vector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub kind: ReservationKind,
    pub column_number: u8,
    pub bytes: u64,
}

/// Plans durable storage before data is written.  A caller can use a larger
/// forecast for bulk ingest; normal collection creation uses 1,000 records.
#[derive(Debug, Clone, Copy)]
pub struct StorageScheduler {
    record_forecast: u64,
}

impl Default for StorageScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_RECORD_FORECAST)
    }
}

impl StorageScheduler {
    /// Creates a scheduler with a minimum forecast of one record.
    pub fn new(record_forecast: u64) -> Self {
        Self {
            record_forecast: record_forecast.max(1),
        }
    }

    /// Returns a sequence of 1, 2, 4, ... 256 MiB reservations large enough
    /// for `required_bytes`.  Requests above 256 MiB are split into max-sized
    /// extents instead of exceeding the file-format limit.
    pub fn binary_reservations(&self, required_bytes: u64) -> Vec<u64> {
        let mut remaining = required_bytes.max(MIN_RESERVATION_BYTES);
        let mut result = Vec::new();
        while remaining > 0 {
            let size = reservation_tier(remaining);
            result.push(size);
            remaining = remaining.saturating_sub(size);
        }
        result
    }

    /// Reserves one row-store area plus one dedicated area for every vector
    /// column. Variable-sized scalar values use a conservative 256-byte
    /// forecast so tables can be scheduled before their first insert.
    pub fn plan_collection(&self, schema: &Schema) -> Vec<Reservation> {
        let row_bytes = schema
            .fields()
            .iter()
            .map(estimated_field_bytes)
            .sum::<u64>()
            .saturating_add(32); // row id, null bitmap, and record framing
        let mut reservations = self
            .binary_reservations(row_bytes.saturating_mul(self.record_forecast))
            .into_iter()
            .map(|bytes| Reservation {
                kind: ReservationKind::Table,
                column_number: 0,
                bytes,
            })
            .collect::<Vec<_>>();

        for field in schema.fields() {
            if let DataType::Vector(vector) = &field.data_type {
                let required = vector_bytes(vector.element_type, vector.dimension)
                    .saturating_mul(self.record_forecast);
                reservations.extend(self.binary_reservations(required).into_iter().map(|bytes| {
                    Reservation {
                        kind: ReservationKind::Vector,
                        column_number: field.column_number,
                        bytes,
                    }
                }));
            }
        }
        reservations
    }

    /// Returns the standard size for a new extent belonging to a collection
    /// data stream. Rollover uses the same plan as initial allocation so the
    /// storage layer can grow without callers managing physical segments.
    pub fn extension_bytes(
        &self,
        schema: &Schema,
        kind: ReservationKind,
        column_number: u8,
        used_bytes: u64,
        allocated_bytes: u64,
    ) -> Option<u64> {
        let baseline = self
            .plan_collection(schema)
            .into_iter()
            .find(|reservation| {
                reservation.kind == kind && reservation.column_number == column_number
            })
            .map(|reservation| reservation.bytes)?;
        // Double the observed stream target on rollover. This gives hot
        // tables progressively larger extents while cold streams retain the
        // conservative initial reservation.
        let target = used_bytes
            .max(allocated_bytes)
            .max(baseline)
            .saturating_mul(2);
        let required = target.max(baseline);
        self.binary_reservations(required).into_iter().next()
    }
}

fn reservation_tier(required_bytes: u64) -> u64 {
    let mut tier = MIN_RESERVATION_BYTES;
    while tier < required_bytes && tier < MAX_RESERVATION_BYTES {
        tier *= 2;
    }
    tier
}

fn estimated_field_bytes(field: &crate::catalog::Field) -> u64 {
    match &field.data_type {
        DataType::Vector(_) => 0, // vector data lives in its own reservation
        DataType::Scalar(ScalarType::UInt8 | ScalarType::Int8 | ScalarType::Boolean) => 1,
        DataType::Scalar(ScalarType::UInt16 | ScalarType::Int16 | ScalarType::Float16) => 2,
        DataType::Scalar(ScalarType::UInt32 | ScalarType::Int32 | ScalarType::Float32) => 4,
        DataType::Scalar(
            ScalarType::UInt64 | ScalarType::Int64 | ScalarType::Float64 | ScalarType::DateTime,
        ) => 8,
        DataType::Scalar(ScalarType::Float128) => 16,
        DataType::Scalar(ScalarType::Text | ScalarType::Bytes) => 256,
    }
}

fn vector_bytes(element: VectorElementType, dimension: u32) -> u64 {
    let bits_per_element = match element {
        VectorElementType::Binary => 1,
        VectorElementType::Int4 => 4,
        VectorElementType::Int8 => 8,
        VectorElementType::Int16 | VectorElementType::FP16 => 16,
        VectorElementType::FP32 => 32,
        VectorElementType::FP64 => 64,
    };
    u64::from(dimension)
        .saturating_mul(bits_per_element)
        .saturating_add(7)
        / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_binary_mib_tiers() {
        let scheduler = StorageScheduler::default();
        assert_eq!(scheduler.binary_reservations(1), vec![MIB]);
        assert_eq!(scheduler.binary_reservations(MIB + 1), vec![2 * MIB]);
        assert_eq!(
            scheduler.binary_reservations(300 * MIB),
            vec![256 * MIB, 64 * MIB]
        );
    }

    #[test]
    fn grows_extensions_from_observed_stream_usage() {
        let scheduler = StorageScheduler::default();
        let schema = Schema::new(vec![crate::catalog::Field {
            column_number: 0,
            name: "body".into(),
            data_type: DataType::Scalar(ScalarType::Text),
            nullable: false,
            primary_key: false,
            auto_increment: false,
        }]);
        assert_eq!(
            scheduler.extension_bytes(&schema, ReservationKind::Table, 0, MIB, MIB),
            Some(2 * MIB)
        );
        assert_eq!(
            scheduler.extension_bytes(&schema, ReservationKind::Table, 0, 3 * MIB, 3 * MIB),
            Some(8 * MIB)
        );
    }
}
