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

/// `f` over `parts` consecutive, about equal slices of `items`, results in order (for work whose
/// results combine the same however the items are divided).
pub fn split<T: Sync, U: Send>(items: &[T], parts: usize, f: impl Fn(&[T]) -> U + Sync + Send) -> Vec<U> {
    let size = items.len().div_ceil(parts).max(1);
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        items.par_chunks(size).map(f).collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        items.chunks(size).map(f).collect()
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

/// `f` on each item, results in order.
#[cfg(feature = "webp")] // its only user
pub fn map<T: Sync, U: Send>(items: &[T], f: impl Fn(&T) -> U + Sync + Send) -> Vec<U> {
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        items.par_iter().map(f).collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        items.iter().map(f).collect()
    }
}

/// `a` and `b`, at once where there are threads.
#[cfg(feature = "webp")] // its only user
pub fn join<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    #[cfg(feature = "parallel")]
    {
        rayon::join(a, b)
    }
    #[cfg(not(feature = "parallel"))]
    {
        (a(), b())
    }
}
