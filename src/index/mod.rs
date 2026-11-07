//! In-memory vector indexes.
//!
//! Indexes return candidate row IDs only. The execution layer still performs
//! exact metric scoring and result ordering, so index implementations must
//! never mutate rows or become the source of truth for stored data.

mod hnsw;
mod ivf;

use crate::types::DistanceMetric;

pub(crate) use hnsw::HnswIndex;
pub(crate) use ivf::IvfIndex;

#[derive(Debug, Clone)]
pub(crate) enum VectorIndex {
    Flat,
    Hnsw(HnswIndex),
    Ivf(IvfIndex),
}

impl VectorIndex {
    pub(crate) fn build(
        kind: &str,
        points: &[(u64, Vec<f32>)],
        metric: DistanceMetric,
        m: usize,
        ef_construction: usize,
        nlist: usize,
    ) -> Result<Self, String> {
        match kind {
            "flat" => Ok(Self::Flat),
            "hnsw" => Ok(Self::Hnsw(HnswIndex::build(
                points,
                metric,
                m,
                ef_construction,
            ))),
            "ivf" => Ok(Self::Ivf(IvfIndex::build(points, metric, nlist))),
            other => Err(format!("unknown vector index type '{other}'")),
        }
    }

    pub(crate) fn candidates(
        &self,
        query: &[f32],
        limit: usize,
        search_budget: usize,
    ) -> Option<Vec<u64>> {
        match self {
            Self::Flat => None,
            Self::Hnsw(index) => Some(index.search(query, limit, search_budget)),
            Self::Ivf(index) => Some(index.search(query, limit, search_budget)),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Hnsw(_) => "hnsw",
            Self::Ivf(_) => "ivf",
        }
    }

    pub(crate) fn parameters(&self) -> (usize, usize, usize) {
        match self {
            Self::Flat => (16, 200, 16),
            Self::Hnsw(index) => index.parameters(),
            Self::Ivf(index) => index.parameters(),
        }
    }
}

pub(crate) fn distance(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        DistanceMetric::Cosine => {
            let dot = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
            let an = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let bn = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            if an == 0.0 || bn == 0.0 {
                1.0
            } else {
                1.0 - dot / (an * bn)
            }
        }
        DistanceMetric::Euclidean => a
            .iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt(),
        DistanceMetric::DotProduct => -a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>(),
    }
}
