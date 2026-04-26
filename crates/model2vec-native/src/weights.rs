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

    /// Return a `&[f32]` row of length `cols`.
    pub(crate) fn row(&self, row: usize) -> &[f32] {
        match self {
            Weights::Mmap {
                _mmap,
                offset,
                len,
                cols,
            } => {
                let start = row * *cols;
                debug_assert!(start + *cols <= *len);
                // Reconstitute the &[f32] view from the mmap. We do
                // this on every call rather than caching a slice in the
                // struct because that slice's lifetime would have to
                // be tied to the Mmap's lifetime, which fights the
                // ownership model.
                //
                // Cost is two pointer adds + a length compute — ~ns.
                let bytes = &_mmap[*offset..*offset + *len * std::mem::size_of::<f32>()];
                let floats: &[f32] = bytemuck::cast_slice(bytes);
                &floats[start..start + *cols]
            }
            Weights::Heap { data, cols } => {
                let start = row * *cols;
                &data[start..start + *cols]
            }
        }
    }
}
