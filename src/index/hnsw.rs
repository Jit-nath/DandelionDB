//! Small in-memory HNSW-style neighbor graph.
//!
//! This implementation is dependency-free and returns approximate candidate
//! IDs; exact ranking remains in the execution layer.

use super::distance;
use crate::types::DistanceMetric;

#[derive(Debug, Clone)]
pub(crate) struct HnswIndex {
    points: Vec<(u64, Vec<f32>)>,
    neighbors: Vec<Vec<usize>>,
    metric: DistanceMetric,
    m: usize,
    ef_construction: usize,
}

impl HnswIndex {
    pub(crate) fn build(
        points: &[(u64, Vec<f32>)],
        metric: DistanceMetric,
        m: usize,
        _ef_construction: usize,
    ) -> Self {
        let m = m.max(1);
        let mut neighbors = vec![Vec::new(); points.len()];
        for index in 0..points.len() {
            let mut nearest = (0..index)
                .map(|other| (distance(metric, &points[index].1, &points[other].1), other))
                .collect::<Vec<_>>();
            nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
            nearest.truncate(m);
            neighbors[index] = nearest.iter().map(|(_, other)| *other).collect();
            for (_, other) in nearest {
                neighbors[other].push(index);
                neighbors[other].sort_unstable();
                neighbors[other].dedup();
                if neighbors[other].len() > m {
                    neighbors[other].truncate(m);
                }
            }
        }
        Self {
            points: points.to_vec(),
            neighbors,
            metric,
            m,
            ef_construction: _ef_construction.max(1),
        }
    }

    pub(crate) fn parameters(&self) -> (usize, usize, usize) {
        (self.m, self.ef_construction, 16)
    }

    pub(crate) fn search(&self, query: &[f32], limit: usize, ef: usize) -> Vec<u64> {
        if self.points.is_empty() {
            return Vec::new();
        }
        let mut candidates = vec![0usize];
        let mut visited = vec![false; self.points.len()];
        let budget = ef.max(limit).max(self.m);
        while let Some(node) = candidates.pop() {
            if visited[node] {
                continue;
            }
            visited[node] = true;
            if visited.iter().filter(|value| **value).count() >= budget {
                break;
            }
            candidates.extend(
                self.neighbors[node]
                    .iter()
                    .copied()
                    .filter(|next| !visited[*next]),
            );
        }
        let mut ranked = visited
            .iter()
            .enumerate()
            .filter(|(_, seen)| **seen)
            .map(|(index, _)| {
                (
                    distance(self.metric, query, &self.points[index].1),
                    self.points[index].0,
                )
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        ranked.truncate(limit);
        ranked.into_iter().map(|(_, id)| id).collect()
    }
}
