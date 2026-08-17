//! SIMD and scalar vector-distance kernels.

use crate::database::model::Metric;

#[inline]
pub fn dot(left: &[f32], right: &[f32]) -> f32 {
    debug_assert_eq!(left.len(), right.len());

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx512f") {
            // SAFETY: the feature is checked at runtime and all loads are
            // bounded by the two slices.
            return unsafe { dot_avx512(left, right) };
        }
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the feature is checked at runtime and the implementation uses
            // unaligned loads bounded by the two slices.
            return unsafe { dot_avx2(left, right) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: Advanced SIMD is part of the AArch64 base architecture.
        return unsafe { dot_neon(left, right) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    dot_scalar(left, right)
}

#[inline]
pub fn squared_l2(left: &[f32], right: &[f32]) -> f32 {
    debug_assert_eq!(left.len(), right.len());

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("avx512f") {
            // SAFETY: the feature is checked at runtime and all loads are
            // bounded by the two slices.
            return unsafe { squared_l2_avx512(left, right) };
        }
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the feature is checked at runtime and the implementation uses
            // unaligned loads bounded by the two slices.
            return unsafe { squared_l2_avx2(left, right) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: Advanced SIMD is part of the AArch64 base architecture.
        return unsafe { squared_l2_neon(left, right) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    squared_l2_scalar(left, right)
}

#[inline]
pub fn norm(vector: &[f32]) -> f32 {
    dot(vector, vector).sqrt()
}

#[inline]
pub fn cost(
    metric: Metric,
    query: &[f32],
    query_norm: f32,
    candidate: &[f32],
    candidate_norm: f32,
) -> f32 {
    match metric {
        Metric::Cosine => -(dot(query, candidate) / (query_norm * candidate_norm)),
        Metric::Dot => -dot(query, candidate),
        Metric::Euclidean => squared_l2(query, candidate),
    }
}

#[inline]
pub fn score_from_cost(metric: Metric, cost: f32) -> f32 {
    match metric {
        Metric::Cosine | Metric::Dot => -cost,
        Metric::Euclidean => cost.sqrt(),
    }
}

#[inline]
#[cfg(not(target_arch = "aarch64"))]
fn dot_scalar(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .fold(0.0, |sum, (&a, &b)| a.mul_add(b, sum))
}

#[inline]
#[cfg(not(target_arch = "aarch64"))]
fn squared_l2_scalar(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).fold(0.0, |sum, (&a, &b)| {
        let delta = a - b;
        delta.mul_add(delta, sum)
    })
}

#[cfg(target_arch = "x86")]
use std::arch::x86::{
    __m256, __m512, _mm256_add_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_setzero_ps,
    _mm256_storeu_ps, _mm256_sub_ps, _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps,
    _mm512_setzero_ps, _mm512_storeu_ps, _mm512_sub_ps,
};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m256, __m512, _mm256_add_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_setzero_ps,
    _mm256_storeu_ps, _mm256_sub_ps, _mm512_add_ps, _mm512_loadu_ps, _mm512_mul_ps,
    _mm512_setzero_ps, _mm512_storeu_ps, _mm512_sub_ps,
};

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::{float32x4_t, vaddq_f32, vld1q_f32, vmulq_f32, vst1q_f32, vsubq_f32};

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
unsafe fn dot_avx512(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    let mut accumulator: __m512 = _mm512_setzero_ps();
    while index + 16 <= left.len() {
        // SAFETY: the loop bounds guarantee sixteen readable f32 values.
        let a = unsafe { _mm512_loadu_ps(left.as_ptr().add(index)) };
        let b = unsafe { _mm512_loadu_ps(right.as_ptr().add(index)) };
        accumulator = _mm512_add_ps(accumulator, _mm512_mul_ps(a, b));
        index += 16;
    }
    let mut lanes = [0.0_f32; 16];
    // SAFETY: lanes has space for sixteen f32 values.
    unsafe { _mm512_storeu_ps(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        sum = left[index].mul_add(right[index], sum);
        index += 1;
    }
    sum
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx512f")]
unsafe fn squared_l2_avx512(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    let mut accumulator: __m512 = _mm512_setzero_ps();
    while index + 16 <= left.len() {
        // SAFETY: the loop bounds guarantee sixteen readable f32 values.
        let a = unsafe { _mm512_loadu_ps(left.as_ptr().add(index)) };
        let b = unsafe { _mm512_loadu_ps(right.as_ptr().add(index)) };
        let delta = _mm512_sub_ps(a, b);
        accumulator = _mm512_add_ps(accumulator, _mm512_mul_ps(delta, delta));
        index += 16;
    }
    let mut lanes = [0.0_f32; 16];
    // SAFETY: lanes has space for sixteen f32 values.
    unsafe { _mm512_storeu_ps(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        let delta = left[index] - right[index];
        sum = delta.mul_add(delta, sum);
        index += 1;
    }
    sum
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn dot_avx2(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    let mut accumulator: __m256 = _mm256_setzero_ps();
    while index + 8 <= left.len() {
        // SAFETY: the loop bounds guarantee eight readable f32 values.
        let a = unsafe { _mm256_loadu_ps(left.as_ptr().add(index)) };
        let b = unsafe { _mm256_loadu_ps(right.as_ptr().add(index)) };
        accumulator = _mm256_add_ps(accumulator, _mm256_mul_ps(a, b));
        index += 8;
    }
    let mut lanes = [0.0_f32; 8];
    // SAFETY: lanes has space for eight f32 values.
    unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        sum = left[index].mul_add(right[index], sum);
        index += 1;
    }
    sum
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn squared_l2_avx2(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    let mut accumulator: __m256 = _mm256_setzero_ps();
    while index + 8 <= left.len() {
        // SAFETY: the loop bounds guarantee eight readable f32 values.
        let a = unsafe { _mm256_loadu_ps(left.as_ptr().add(index)) };
        let b = unsafe { _mm256_loadu_ps(right.as_ptr().add(index)) };
        let delta = _mm256_sub_ps(a, b);
        accumulator = _mm256_add_ps(accumulator, _mm256_mul_ps(delta, delta));
        index += 8;
    }
    let mut lanes = [0.0_f32; 8];
    // SAFETY: lanes has space for eight f32 values.
    unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        let delta = left[index] - right[index];
        sum = delta.mul_add(delta, sum);
        index += 1;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn dot_neon(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    // SAFETY: constructing a zero vector does not access memory.
    let mut accumulator: float32x4_t = unsafe { std::mem::zeroed() };
    while index + 4 <= left.len() {
        // SAFETY: the loop bounds guarantee four readable f32 values.
        let a = unsafe { vld1q_f32(left.as_ptr().add(index)) };
        let b = unsafe { vld1q_f32(right.as_ptr().add(index)) };
        accumulator = vaddq_f32(accumulator, vmulq_f32(a, b));
        index += 4;
    }
    let mut lanes = [0.0_f32; 4];
    // SAFETY: lanes has space for four f32 values.
    unsafe { vst1q_f32(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        sum = left[index].mul_add(right[index], sum);
        index += 1;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn squared_l2_neon(left: &[f32], right: &[f32]) -> f32 {
    let mut index = 0;
    // SAFETY: constructing a zero vector does not access memory.
    let mut accumulator: float32x4_t = unsafe { std::mem::zeroed() };
    while index + 4 <= left.len() {
        // SAFETY: the loop bounds guarantee four readable f32 values.
        let a = unsafe { vld1q_f32(left.as_ptr().add(index)) };
        let b = unsafe { vld1q_f32(right.as_ptr().add(index)) };
        let delta = vsubq_f32(a, b);
        accumulator = vaddq_f32(accumulator, vmulq_f32(delta, delta));
        index += 4;
    }
    let mut lanes = [0.0_f32; 4];
    // SAFETY: lanes has space for four f32 values.
    unsafe { vst1q_f32(lanes.as_mut_ptr(), accumulator) };
    let mut sum = lanes.into_iter().sum::<f32>();
    while index < left.len() {
        let delta = left[index] - right[index];
        sum = delta.mul_add(delta, sum);
        index += 1;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::{dot, squared_l2};

    #[test]
    fn kernels_match_known_values() {
        let left = [1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 5.0];
        let right = [2.0, 3.0, 4.0, 5.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        assert_eq!(dot(&left, &right), 70.0);
        assert_eq!(squared_l2(&left, &right), 5.0);
    }
}
