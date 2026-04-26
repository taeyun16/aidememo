//! Embedding-matrix view backed by either an mmap (zero-copy F32) or a
//! one-time decoded `Vec<f32>` (when the tensor is F16/I8/etc and we
//! have to widen to f32 anyway).

use memmap2::Mmap;
use safetensors::tensor::Dtype;
use std::sync::Arc;

use crate::Error;

/// Backing storage for the matrix. Two flavors:
///
/// - **`Mmap`**: the safetensors payload is f32 little-endian (the
///   common case). On every Rust target that means it's wire-compatible
///   with `&[f32]`, so we hold an Arc to the mmap and re-interpret the
///   bytes as `&[f32]` on demand. Cost: zero allocations, lazy paging.
///
/// - **`Heap`**: the tensor is F16 / I8 / something else. We have to
///   widen to f32 once at load time. Cost: one allocation, but the
///   storage is half / a quarter the f32 size — heap-friendly already.
pub(crate) enum Weights {
    Mmap {
        // Keep the mmap alive — `data` borrows from it.
        _mmap: Arc<Mmap>,
        // Byte offset into the mmap where the f32 payload starts.
        offset: usize,
        // Number of f32 elements (rows * cols).
        len: usize,
        cols: usize,
    },
    Heap {
        data: Vec<f32>,
        cols: usize,
    },
    /// Load-time int8 quantization: each row scaled to fit in [-127, 127]
    /// with its own f32 scale (per-row max-abs). Cuts the f32 489 MB
    /// matrix to ~122 MB heap + ~2 MB of per-row scales. mmap zero-copy
    /// is given up here; the trade-off is "smaller heap" vs "no copy".
    /// Per-row (not global) scale because models2vec embeddings have
    /// outlier dimensions — global scaling would crush dynamic range
    /// for the median row.
    QuantizedI8 {
        data: Vec<i8>,
        scales: Vec<f32>,
        cols: usize,
    },
}

impl Weights {
    pub(crate) fn from_tensor(
        dtype: Dtype,
        raw: &[u8],
        mmap: &Arc<Mmap>,
        rows: usize,
        cols: usize,
    ) -> Result<Self, Error> {
        match dtype {
            Dtype::F32 => {
                // Locate `raw`'s offset inside the mmap. SafeTensors
                // hands us a slice into the same buffer we passed in,
                // so pointer arithmetic over the mmap base recovers
                // the offset exactly. This dodges another lookup.
                let mmap_base = mmap.as_ptr() as usize;
                let raw_base = raw.as_ptr() as usize;
                debug_assert!(raw_base >= mmap_base);
                let offset = raw_base.saturating_sub(mmap_base);

                // Sanity check that the slice fits inside the mmap.
                if offset + raw.len() > mmap.len() {
                    return Err(Error::SafeTensors(
                        "tensor data offset escapes mmap region".into(),
                    ));
                }
                if raw.len() != rows * cols * std::mem::size_of::<f32>() {
                    return Err(Error::SafeTensors(format!(
                        "F32 tensor size mismatch: expected {} bytes for {}x{}, got {}",
                        rows * cols * 4,
                        rows,
                        cols,
                        raw.len()
                    )));
                }
                // Alignment: bytemuck requires 4-byte alignment for
                // &[f32]. SafeTensors' header rounds to multiples of 8
                // by spec, so this holds in practice — but we verify
                // and fall back to a heap copy if the platform / file
                // ever violates it.
                if (raw_base % std::mem::align_of::<f32>()) != 0 {
                    let widened: Vec<f32> = raw
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    return Ok(Weights::Heap {
                        data: widened,
                        cols,
                    });
                }
                Ok(Weights::Mmap {
                    _mmap: mmap.clone(),
                    offset,
                    len: rows * cols,
                    cols,
                })
            }
            Dtype::F16 => {
                let widened: Vec<f32> = raw
                    .chunks_exact(2)
                    .map(|b| half::f16::from_le_bytes([b[0], b[1]]).to_f32())
                    .collect();
                Ok(Weights::Heap {
                    data: widened,
                    cols,
                })
            }
            Dtype::I8 => {
                let widened: Vec<f32> = raw.iter().map(|&b| f32::from(b as i8)).collect();
                Ok(Weights::Heap {
                    data: widened,
                    cols,
                })
            }
            other => Err(Error::UnsupportedDtype(other)),
        }
    }

    /// A row, ready to be accumulated into an f32 mean-pool buffer.
    /// Returns either a borrowed `&[f32]` (no work) or an owned i8
    /// slice + scale that the caller widens at accumulate time. The
    /// enum keeps the hot path branch-light — pool() dispatches once
    /// per call, then the inner row loop is monomorphic.
    pub(crate) fn row<'a>(&'a self, row: usize) -> RowView<'a> {
        match self {
            Weights::Mmap {
                _mmap,
                offset,
                len,
                cols,
            } => {
                let start = row * *cols;
                debug_assert!(start + *cols <= *len);
                let bytes = &_mmap[*offset..*offset + *len * std::mem::size_of::<f32>()];
                let floats: &[f32] = bytemuck::cast_slice(bytes);
                RowView::F32(&floats[start..start + *cols])
            }
            Weights::Heap { data, cols } => {
                let start = row * *cols;
                RowView::F32(&data[start..start + *cols])
            }
            Weights::QuantizedI8 { data, scales, cols } => {
                let start = row * *cols;
                RowView::I8 {
                    row: &data[start..start + *cols],
                    scale: scales.get(row).copied().unwrap_or(0.0),
                }
            }
        }
    }

    /// Quantize the matrix held by `self` to int8 with per-row max-abs
    /// scaling, returning a new `QuantizedI8` form. Critically, this
    /// reads each row directly from the source storage — *without*
    /// materializing a staging f32 buffer. So the temporary memory cost
    /// is just the destination i8 matrix (~rows*cols bytes) plus the
    /// scales vector, never an extra rows*cols*4 bytes f32 copy.
    ///
    /// Quantization rule per row:
    ///   max = max(|row[i]|)
    ///   scale = max / 127           (so the f32 magnitude is recoverable)
    ///   q[i]  = round(row[i] / scale).clamp(-127, 127)
    ///
    /// Recovery: `row_f32[i] ≈ q[i] * scale`. Error bound per element
    /// is roughly `max / 254` — a 1/254 fraction of the row's largest
    /// magnitude. For unit-normalized embedding rows that's around
    /// 0.4% per dimension; cosine similarity with another quantized
    /// row stays within ~0.5% of the f32 ground truth in practice.
    pub(crate) fn quantize_in_place(&self, rows: usize, cols: usize) -> Weights {
        let mut data = vec![0i8; rows * cols];
        let mut scales = vec![0.0f32; rows];
        for r in 0..rows {
            // Pull the row from whichever storage we have. F32 path is
            // a slice of the mmap (zero-copy); I8 path widens inline.
            // The widened f32 view never escapes this loop iteration.
            let dst = &mut data[r * cols..(r + 1) * cols];
            match self.row(r) {
                RowView::F32(src) => quantize_row_f32(src, dst, &mut scales[r]),
                RowView::I8 { row, scale } => quantize_row_i8(row, scale, dst, &mut scales[r]),
            }
        }
        Weights::QuantizedI8 { data, scales, cols }
    }
}

/// Quantize one f32 row into a pre-allocated i8 slot. Pure function;
/// hoisted so the loop in `quantize_in_place` is mono-typed.
fn quantize_row_f32(src: &[f32], dst: &mut [i8], scale_out: &mut f32) {
    let max = src.iter().fold(0f32, |acc, x| acc.max(x.abs()));
    if max == 0.0 {
        *scale_out = 0.0;
        return;
    }
    let scale = max / 127.0;
    *scale_out = scale;
    let inv = 1.0 / scale;
    for (q, &v) in dst.iter_mut().zip(src.iter()) {
        *q = (v * inv).round().clamp(-127.0, 127.0) as i8;
    }
}

/// Same, when the input row is already i8 (with an existing scale).
/// We re-quantize anyway to a uniform output shape; the only
/// information lost vs the original was already lost when the source
/// was quantized upstream.
fn quantize_row_i8(src: &[i8], src_scale: f32, dst: &mut [i8], scale_out: &mut f32) {
    // The widened max-abs is just the i8 max-abs times src_scale.
    let max_q = src
        .iter()
        .map(|&v| v.unsigned_abs() as i32)
        .max()
        .unwrap_or(0);
    if max_q == 0 {
        *scale_out = 0.0;
        return;
    }
    let max = max_q as f32 * src_scale;
    let scale = max / 127.0;
    *scale_out = scale;
    let inv = 1.0 / scale;
    for (q, &v) in dst.iter_mut().zip(src.iter()) {
        let f = (v as f32) * src_scale;
        *q = (f * inv).round().clamp(-127.0, 127.0) as i8;
    }
}

/// One row, in whichever form the storage holds. The pool loop in
/// lib.rs is the only consumer; it dispatches once and then runs a
/// tight inner loop.
pub(crate) enum RowView<'a> {
    F32(&'a [f32]),
    I8 {
        row: &'a [i8],
        /// `i8 -> f32` recovery scale. Multiply each element by this
        /// before accumulating.
        scale: f32,
    },
}
