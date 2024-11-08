use crate::qjs;
use std::ptr::null_mut;

#[cfg(feature = "quickjs-libc-test")]
mod global;
#[cfg(feature = "quickjs-libc-test")]
pub use global::{get_global_used_memory_size, memory_size_dec, memory_size_inc};
#[cfg(all(feature = "quickjs-libc", feature = "allocator", not(feature = "quickjs-libc-test")))]
mod qjs_wasm;
#[cfg(all(feature = "quickjs-libc", feature = "allocator", not(feature = "quickjs-libc-test")))]
pub use qjs_wasm::{QuickjsWasmAllocator, set_is_main_runtime, memory_size_dec, memory_size_inc, is_main_runtime};
mod rust;
pub use rust::RustAllocator;

/// Raw memory pointer
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "allocator")))]
pub type RawMemPtr = *mut u8;

/// The allocator interface
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "allocator")))]
pub trait Allocator {
    /// Allocate new memory
    fn alloc(&mut self, size: usize) -> RawMemPtr;

    /// De-allocate previously allocated memory
    fn dealloc(&mut self, ptr: RawMemPtr);

    /// Re-allocate previously allocated memory
    fn realloc(&mut self, ptr: RawMemPtr, new_size: usize) -> RawMemPtr;

    /// Get usable size of allocated memory region
    fn usable_size(ptr: RawMemPtr) -> usize
    where
        Self: Sized;

	#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
	fn trigger_gc(&self, _size: usize) -> i32 {
		0
	}

	#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
	fn set_gc_threshold(&mut self, _size: usize) {
		unreachable!()
	}

	#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
	fn enlarge_gc_threshold(&mut self) {
		unreachable!()
	}
}

type DynAllocator = Box<dyn Allocator>;

#[repr(transparent)]
pub struct AllocatorHolder(*mut DynAllocator);

#[cfg(not(feature = "quickjs-libc"))]
impl Drop for AllocatorHolder {
    fn drop(&mut self) {
        let _ = unsafe { Box::from_raw(self.0) };
    }
}

impl AllocatorHolder {
    pub(crate) fn functions<A>() -> qjs::JSMallocFunctions
    where
        A: Allocator,
    {
        qjs::JSMallocFunctions {
            js_malloc: Some(Self::malloc),
            js_free: Some(Self::free),
            js_realloc: Some(Self::realloc),
            js_malloc_usable_size: Some(Self::malloc_usable_size::<A>),
			#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
			js_force_gc: Some(Self::trigger_gc),
			#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
			js_set_gc_threshold: Some(Self::set_gc_threshold),
			#[cfg(all(feature = "quickjs-libc", feature = "allocator"))]
			js_enlarge_gc_threshold: Some(Self::enlarge_gc_threshold),
        }
    }

    pub(crate) fn new<A>(allocator: A) -> Self
    where
        A: Allocator + 'static,
    {
        Self(Box::into_raw(Box::new(Box::new(allocator))))
    }

    pub(crate) fn opaque_ptr(&self) -> *mut DynAllocator {
        self.0
    }

	#[cfg(feature = "allocator")]
	unsafe extern "C" fn trigger_gc(
        state: *mut qjs::JSMallocState,
        size: qjs::size_t,
    ) -> qjs::c_int {
        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

		allocator.trigger_gc(size as _) as _
    }

	#[cfg(feature = "allocator")]
	unsafe extern "C" fn set_gc_threshold(
        state: *mut qjs::JSMallocState,
        size: qjs::size_t,
    ) {
        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

		allocator.set_gc_threshold(size as _);
    }

	#[cfg(feature = "allocator")]
	unsafe extern "C" fn enlarge_gc_threshold(state: *mut qjs::JSMallocState) {
        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

		allocator.enlarge_gc_threshold();
    }

    unsafe extern "C" fn malloc(
        state: *mut qjs::JSMallocState,
        size: qjs::size_t,
    ) -> *mut qjs::c_void {
        // simulate the default behavior of libc::malloc
        if size == 0 {
            return null_mut();
        }

        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

		allocator.alloc(size as _) as _
    }

    unsafe extern "C" fn free(state: *mut qjs::JSMallocState, ptr: *mut qjs::c_void) {
        // simulate the default behavior of libc::free
        if ptr.is_null() {
            // nothing to do
            return;
        }

        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

        allocator.dealloc(ptr as _);
    }

    unsafe extern "C" fn realloc(
        state: *mut qjs::JSMallocState,
        ptr: *mut qjs::c_void,
        size: qjs::size_t,
    ) -> *mut qjs::c_void {
        let state = &*state;
        let allocator = &mut *(state.opaque as *mut DynAllocator);

        // simulate the default behavior of libc::realloc
        if ptr.is_null() && size != 0 {
            // alloc new memory chunk
            return allocator.alloc(size as _) as _;
        } else if size == 0 {
            // free memory chunk
			if !ptr.is_null() {
				allocator.dealloc(ptr as _);
			}
            return null_mut();
        }

        allocator.realloc(ptr as _, size as _) as _
    }

    unsafe extern "C" fn malloc_usable_size<A>(ptr: *const qjs::c_void) -> qjs::size_t
    where
        A: Allocator,
    {
        // simulate the default bahavior of libc::malloc_usable_size
        if ptr.is_null() {
            return 0;
        }
        A::usable_size(ptr as _) as _
    }
}
