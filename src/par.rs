//! Work split into fixed chunks, run in parallel with the `parallel` feature and combined in order,
//! so results don't depend on the number of threads (or on having threads at all).

/// Items per chunk for `chunked`.
pub const CHUNK: usize = 1 << 14;

/// `f` over consecutive CHUNK-item chunks of `items` (with the chunk's offset), results in order.
pub fn chunked<T: Sync, U: Send>(items: &[T], f: impl Fn(usize, &[T]) -> U + Sync + Send) -> Vec<U> {
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        items.par_chunks(CHUNK).enumerate().map(|(i, c)| f(i * CHUNK, c)).collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        items.chunks(CHUNK).enumerate().map(|(i, c)| f(i * CHUNK, c)).collect()
    }
}

/// `f` on consecutive `size`-item chunks of `out` (with the chunk's offset).
pub fn fill<T: Send>(out: &mut [T], size: usize, f: impl Fn(usize, &mut [T]) + Sync + Send) {
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        out.par_chunks_mut(size).enumerate().for_each(|(i, c)| f(i * size, c));
    }
    #[cfg(not(feature = "parallel"))]
    {
        out.chunks_mut(size).enumerate().for_each(|(i, c)| f(i * size, c));
    }
}
