use std::{alloc::System, sync::atomic::{AtomicUsize, Ordering}};
use crate::{Allocator, RawMemPtr};

use super::RustAllocator;

static GLOBAL_TOTAL_SIZE: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_PEAK: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_THRESHOLD: AtomicUsize = AtomicUsize::new(256 * 1024);

#[global_allocator]
static GLOBAL_ALLOCATOR: RustGlobalAlloc = RustGlobalAlloc {
	allocator: RustAllocator,
};

struct RustGlobalAlloc {
	allocator: RustAllocator,
}

impl Allocator for RustGlobalAlloc {
	fn alloc(&mut self, size: usize) -> RawMemPtr {
		self.allocator.alloc(size)
	}

	fn dealloc(&mut self, ptr: RawMemPtr) {
		self.allocator.dealloc(ptr);
	}

	fn realloc(&mut self, ptr: RawMemPtr, new_size: usize) -> RawMemPtr {
		self.allocator.realloc(ptr, new_size)
	}

	fn usable_size(ptr: RawMemPtr) -> usize
		where
			Self: Sized {
		RustAllocator::usable_size(ptr)
	}

	fn set_gc_threshold(&mut self, size: usize) {
		GLOBAL_THRESHOLD.store(size, Ordering::Relaxed);
	}

	fn trigger_gc(&self, size: usize) -> i32 {
		let now = GLOBAL_TOTAL_SIZE.load(Ordering::Relaxed);
		let threshold = GLOBAL_THRESHOLD.load(Ordering::Relaxed);
		(now + size > threshold) as i32
	}

	fn enlarge_gc_threshold(&mut self) {
		let now = GLOBAL_TOTAL_SIZE.load(Ordering::Relaxed);
		GLOBAL_THRESHOLD.store(now + (now >> 1), Ordering::Relaxed);
	}
}

unsafe impl std::alloc::GlobalAlloc for RustGlobalAlloc {
	unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
		let prev = GLOBAL_TOTAL_SIZE.fetch_add(layout.size(), Ordering::Relaxed);
		GLOBAL_PEAK.fetch_max(prev + layout.size(), Ordering::Relaxed);
		System.alloc(layout)
	}

	unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
		GLOBAL_TOTAL_SIZE.fetch_sub(layout.size(), Ordering::Relaxed);
		System.dealloc(ptr, layout);
	}

	unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { std::alloc::Layout::from_size_align_unchecked(new_size, layout.align()) };
        let new_ptr = unsafe { System.alloc(new_layout) };
        if !new_ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, new_ptr, std::cmp::min(layout.size(), new_size));
                System.dealloc(ptr, layout);
            }
        }
		GLOBAL_TOTAL_SIZE.fetch_sub(layout.size(), Ordering::Relaxed);
		let prev = GLOBAL_TOTAL_SIZE.fetch_add(new_layout.size(), Ordering::Relaxed);
		GLOBAL_PEAK.fetch_max(prev + new_layout.size(), Ordering::Relaxed);
        new_ptr
	}
}

pub fn memory_size_inc(size: usize) {
	let prev = GLOBAL_TOTAL_SIZE.fetch_add(size, Ordering::Relaxed);
	GLOBAL_PEAK.fetch_max(prev + size, Ordering::Relaxed);
}

pub fn memory_size_dec(size: usize) {
	GLOBAL_TOTAL_SIZE.fetch_sub(size, Ordering::Relaxed);
}

pub fn get_global_used_memory_size() -> usize {
	GLOBAL_PEAK.load(Ordering::Relaxed)
}
