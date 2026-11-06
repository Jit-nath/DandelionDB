//! Small in-memory inverted-file vector index.
//!
//! IVF assigns each vector to a deterministic centroid. Queries probe nearby
//! lists and return candidate IDs for exact reranking.

use super::distance;
use crate::types::DistanceMetric;

#[derive(Debug, Clone)]
pub(crate) struct IvfIndex {
    centroids: Vec<Vec<f64>>,
    lists: Vec<Vec<(u64, Vec<f64>)>>,
    metric: DistanceMetric,
}

impl IvfIndex {
    pub(crate) fn build(points: &[(u64, Vec<f64>)], metric: DistanceMetric, nlist: usize) -> Self {
        if points.is_empty() {
            return Self {
                centroids: Vec::new(),
                lists: Vec::new(),
                metric,
            };
        }
        let count = nlist.max(1).min(points.len());
        let centroids = points
            .iter()
            .take(count)
            .map(|(_, vector)| vector.clone())
            .collect::<Vec<_>>();
        let mut lists = vec![Vec::new(); count];
        for (id, vector) in points {
            let bucket = centroids
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    distance(metric, vector, a).total_cmp(&distance(metric, vector, b))
                })
                .map(|(index, _)| index)
                .unwrap_or(0);
            lists[bucket].push((*id, vector.clone()));
        }
        Self {
            centroids,
            lists,
            metric,
        }
    }

    pub(crate) fn parameters(&self) -> (usize, usize, usize) {
        (16, 200, self.centroids.len().max(1))
    }

    pub(crate) fn search(&self, query: &[f64], limit: usize, nprobe: usize) -> Vec<u64> {
        if self.centroids.is_empty() {
            return Vec::new();
        }
        let mut buckets = (0..self.centroids.len())
            .map(|index| (distance(self.metric, query, &self.centroids[index]), index))
            .collect::<Vec<_>>();
        buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
        buckets.truncate(nprobe.max(1).min(buckets.len()));
        let mut ranked = buckets
            .into_iter()
            .flat_map(|(_, bucket)| self.lists[bucket].iter())
            .map(|(id, vector)| (distance(self.metric, query, vector), *id))
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        ranked.truncate(limit);
        ranked.into_iter().map(|(_, id)| id).collect()
    }
}
