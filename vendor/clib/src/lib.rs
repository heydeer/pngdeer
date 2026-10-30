//! The C library functions pngdeer's vendored C code (vendor/libwebp, vendor/mozjpeg) calls, where
//! they would differ between targets or wasm32 has none: the inexact math functions and qsort on
//! every target, and the allocator on wasm32 (the rest of what wasm32 needs is libc.c). The crates
//! that use it link it with `use pngdeer_clib as _;`.

use std::ffi::{c_int, c_void};

// The math functions and qsort the vendored libraries call on every target (include/pngdeer_clib.h):
// one implementation, so they give the same bits everywhere.

#[no_mangle]
extern "C" fn pngdeer_log(x: f64) -> f64 {
    libm::log(x)
}

#[no_mangle]
extern "C" fn pngdeer_log2(x: f64) -> f64 {
    libm::log2(x)
}

#[no_mangle]
extern "C" fn pngdeer_log10(x: f64) -> f64 {
    libm::log10(x)
}

#[no_mangle]
extern "C" fn pngdeer_exp(x: f64) -> f64 {
    libm::exp(x)
}

#[no_mangle]
extern "C" fn pngdeer_pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[no_mangle]
extern "C" fn pngdeer_logf(x: f32) -> f32 {
    libm::logf(x)
}

#[no_mangle]
extern "C" fn pngdeer_expf(x: f32) -> f32 {
    libm::expf(x)
}

/// qsort as a stable sort: equal elements keep their order, whatever the platform.
///
/// # Safety
/// As qsort: `base` holds `count` elements of `size` bytes, and `compare` orders them.
#[no_mangle]
unsafe extern "C" fn pngdeer_qsort(base: *mut c_void, count: usize, size: usize, compare: extern "C" fn(*const c_void, *const c_void) -> c_int) {
    if count < 2 || size == 0 {
        return;
    }
    let bytes = std::slice::from_raw_parts_mut(base.cast::<u8>(), count * size);
    let element = |i: usize| bytes.as_ptr().add(i * size).cast::<c_void>();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| compare(element(a), element(b)).cmp(&0));
    let sorted: Vec<u8> = order.iter().flat_map(|&i| bytes[i * size..(i + 1) * size].iter().copied()).collect();
    bytes.copy_from_slice(&sorted);
}

// wasm32 has no C library: the vendored libraries' allocations go to Rust's allocator, with the size kept in a
// header in front of each block for free and realloc.
#[cfg(target_arch = "wasm32")]
mod alloc {
    use std::alloc::{alloc, dealloc, realloc as grow, Layout};
    use std::ffi::c_void;

    const HEADER: usize = 16; // keeps blocks 16-byte aligned, as malloc's are

    fn layout(size: usize) -> Option<Layout> {
        Layout::from_size_align(size.checked_add(HEADER)?, HEADER).ok()
    }

    #[no_mangle]
    unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
        let Some(layout) = layout(size) else { return std::ptr::null_mut() };
        let block = alloc(layout);
        if block.is_null() {
            return block.cast();
        }
        block.cast::<usize>().write(size);
        block.add(HEADER).cast()
    }

    #[no_mangle]
    unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
        let Some(total) = count.checked_mul(size) else { return std::ptr::null_mut() };
        let pointer = malloc(total);
        if !pointer.is_null() {
            pointer.cast::<u8>().write_bytes(0, total);
        }
        pointer
    }

    #[no_mangle]
    unsafe extern "C" fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
        if pointer.is_null() {
            return malloc(size);
        }
        let block = pointer.cast::<u8>().sub(HEADER);
        let old = block.cast::<usize>().read();
        let (Some(old_layout), Some(_)) = (layout(old), layout(size)) else { return std::ptr::null_mut() };
        let block = grow(block, old_layout, size + HEADER);
        if block.is_null() {
            return block.cast();
        }
        block.cast::<usize>().write(size);
        block.add(HEADER).cast()
    }

    #[no_mangle]
    unsafe extern "C" fn free(pointer: *mut c_void) {
        if pointer.is_null() {
            return;
        }
        let block = pointer.cast::<u8>().sub(HEADER);
        let size = block.cast::<usize>().read();
        dealloc(block, layout(size).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qsort_is_stable() {
        extern "C" fn by_first(a: *const c_void, b: *const c_void) -> c_int {
            // SAFETY: called by pngdeer_qsort on elements of [u8; 2]
            unsafe { (*a.cast::<[u8; 2]>())[0] as c_int - (*b.cast::<[u8; 2]>())[0] as c_int }
        }
        let mut items = [[2u8, 0], [1, 1], [2, 2], [1, 3], [0, 4]];
        unsafe { pngdeer_qsort(items.as_mut_ptr().cast(), items.len(), 2, by_first) };
        assert_eq!(items, [[0, 4], [1, 1], [1, 3], [2, 0], [2, 2]]);
    }
}
