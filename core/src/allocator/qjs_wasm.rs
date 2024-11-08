use crate::{RustAllocator, RawMemPtr, Allocator};
use std::{alloc::{GlobalAlloc, System}, cell::Cell, sync::atomic::{AtomicUsize, Ordering}};

static GLOBAL_TOTAL_SIZE: AtomicUsize = AtomicUsize::new(0);
thread_local! {
	static IS_MAIN_RUNTIME: Cell<bool> = Cell::new(true);
	static THREAD_LOCAL_MEM: AtomicUsize = AtomicUsize::new(0);
}

pub struct QuickjsWasmAllocator {
	allocator: RustAllocator,
	// 仅在quickjs端被修改，每个worker和主线程持有一个独立的threshold
	threshold: usize,
}

impl QuickjsWasmAllocator {
	pub fn new() -> Self {
		Self {
			allocator: RustAllocator,
			threshold: 0,
		}
	}
}

impl Allocator for QuickjsWasmAllocator {
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
		self.threshold = size;
	}

	fn trigger_gc(&self, size: usize) -> i32 {
		if IS_MAIN_RUNTIME.get() {
			let now = GLOBAL_TOTAL_SIZE.load(Ordering::Relaxed);
			(now + size > self.threshold) as i32
		} else {
			let now = THREAD_LOCAL_MEM.with(|mem| mem.load(Ordering::Relaxed));
			(now + size > self.threshold) as i32
		}
	}

	fn enlarge_gc_threshold(&mut self) {
		if IS_MAIN_RUNTIME.get() {
			let now = GLOBAL_TOTAL_SIZE.load(Ordering::Relaxed);
			self.threshold = now + (now >> 1);
		} else {
			let now = THREAD_LOCAL_MEM.with(|mem| mem.load(Ordering::Relaxed));
			self.threshold = now + (now >> 1);
		}
	}
}

struct QuickjsWasmGlobalAllocator;

#[global_allocator]
static ALLOCATOR: QuickjsWasmGlobalAllocator = QuickjsWasmGlobalAllocator;

unsafe impl GlobalAlloc for QuickjsWasmGlobalAllocator {
	unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
		let res = System.alloc(layout);
		if !res.is_null() {
			if IS_MAIN_RUNTIME.get() {
				GLOBAL_TOTAL_SIZE.fetch_add(layout.size(), Ordering::Relaxed);
			} else {
				THREAD_LOCAL_MEM.with(|mem| mem.fetch_add(layout.size(), Ordering::Relaxed));
			}
		}
		res
	}

	unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
		System.dealloc(ptr, layout);
		if IS_MAIN_RUNTIME.get() {
			GLOBAL_TOTAL_SIZE.fetch_sub(layout.size(), Ordering::Relaxed);
		} else {
			THREAD_LOCAL_MEM.with(|mem| mem.fetch_sub(layout.size(), Ordering::Relaxed));
		}
	}
}

pub fn set_is_main_runtime(value: bool) {
	IS_MAIN_RUNTIME.set(value);
}

pub fn is_main_runtime() -> bool {
	IS_MAIN_RUNTIME.get()
}

pub fn memory_size_inc(size: usize) {
	if IS_MAIN_RUNTIME.get() {
		GLOBAL_TOTAL_SIZE.fetch_add(size, Ordering::Relaxed);
	} else {
		THREAD_LOCAL_MEM.with(|mem| mem.fetch_add(size, Ordering::Relaxed));
	}
}

pub fn memory_size_dec(size: usize) {
	if IS_MAIN_RUNTIME.get() {
		GLOBAL_TOTAL_SIZE.fetch_sub(size, Ordering::Relaxed);
	} else {
		THREAD_LOCAL_MEM.with(|mem| mem.fetch_sub(size, Ordering::Relaxed));
	}
}
