use crate::{handle_exception, qjs, Ctx, Error, FromJs, IntoJs, Object, Result, Value};
use std::{
    mem::{size_of, ManuallyDrop, MaybeUninit},
    ops::Deref,
    os::raw::c_void,
    slice,
    sync::Arc,
};

/// Rust representation of a javascript object of class ArrayBuffer.
///
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "array-buffer")))]
#[derive(Debug, PartialEq, Clone)]
#[repr(transparent)]
pub struct ArrayBuffer<'js>(pub(crate) Object<'js>);

impl<'js> ArrayBuffer<'js> {
    /// Create array buffer from vector data
    pub fn new<T: Copy>(ctx: Ctx<'js>, src: impl Into<Vec<T>>) -> Result<Self> {
        let src = ManuallyDrop::new(src.into());
        let ptr = src.as_ptr() as *const u8;
        let capacity = src.capacity();
        let size = src.len() * size_of::<T>();
        unsafe {
            let slice = std::slice::from_raw_parts(ptr, size);
            let len = slice.len();
            let ptr = slice.as_ptr();
            Self::new_raw(ctx, ptr, len, capacity, |ptr, len, capacity| {
                let ptr = ptr as *mut T;
                let length = len / size_of::<T>();
                Vec::from_raw_parts(ptr, length, capacity);
            })
        }
    }

    /// Create array buffer from slice
    pub fn new_copy<T: Copy>(ctx: Ctx<'js>, src: impl AsRef<[T]>) -> Result<Self> {
        let src = src.as_ref();
        let ptr = src.as_ptr();
        let size = src.len() * size_of::<T>();

        Ok(Self(Object(unsafe {
            let val = qjs::JS_NewArrayBufferCopy(ctx.ctx, ptr as _, size as _);
            handle_exception(ctx, val)?;
            Value::from_js_value(ctx, val)
        })))
    }

    pub unsafe fn new_shared<T>(
        ctx: Ctx<'js>,
        ptr: *const u8,
        len: usize,
        opaque: Arc<T>,
    ) -> Result<Self> {
        unsafe extern "C" fn mem_dup<T>(opaque: *mut c_void) -> *mut c_void {
            let opaque = Box::from_raw(opaque as *mut Arc<T>);
            let new_opaque = opaque.clone();
            Box::leak(opaque);
            Box::leak(new_opaque) as *mut Arc<T> as _
        }

        unsafe extern "C" fn mem_free<T>(opaque: *mut c_void) {
            let _opaque = Box::from_raw(opaque as *mut Arc<T>);
        }

        let opaque_ptr = Box::leak(Box::new(opaque)) as *mut Arc<T>;

        let obj = Object(unsafe {
            let val = qjs::JS_NewSharedArrayBufferExotic(
                ctx.ctx,
                ptr as _,
                len as _,
                None,
                Some(mem_dup::<T>),
                Some(mem_free::<T>),
                opaque_ptr as _,
            );
            handle_exception(ctx, val)?;
            Value::from_js_value(ctx, val)
        });
        obj.prevent_extensions()?;

        Ok(Self(obj))
    }

    /// Create array buffer from slice
    pub unsafe fn new_raw<T, F>(
        ctx: Ctx<'js>,
        ptr: *const u8,
        len: usize,
        opaque: T,
        callback: F,
    ) -> Result<Self>
    where
        F: FnOnce(*const u8, usize, T),
    {
        struct Opaque<T, F> {
            size: usize,
            inner: T,
            callback: F,
        }

        unsafe extern "C" fn drop_raw<T, F>(
            _rt: *mut qjs::JSRuntime,
            opaque: *mut c_void,
            ptr: *mut c_void,
        ) where
            F: FnOnce(*const u8, usize, T),
        {
            let opaque = Box::from_raw(opaque as *mut Opaque<T, F>);
            let callback = opaque.callback;
            callback(ptr as *const u8, opaque.size, opaque.inner);
        }

        let opaque_ptr = Box::leak(Box::new(Opaque {
            size: len,
            inner: opaque,
            callback,
        })) as *mut Opaque<T, F>;

        Ok(Self(Object(unsafe {
            let val = qjs::JS_NewArrayBuffer(
                ctx.ctx,
                ptr as _,
                len as _,
                Some(drop_raw::<T, F>),
                opaque_ptr as _,
                0,
            );
            handle_exception(ctx, val).map_err(|error| {
                let opaque = Box::from_raw(opaque_ptr);
                let callback = opaque.callback;
                callback(ptr, len, opaque.inner);
                error
            })?;
            Value::from_js_value(ctx, val)
        })))
    }

    /// Get the length of the array buffer in bytes.
    pub fn len(&self) -> usize {
        Self::get_raw(&self.0).expect("Not an ArrayBuffer").0
    }

    /// Returns wether an array buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Detach array buffer
    pub fn detach(&mut self) {
        unsafe { qjs::JS_DetachArrayBuffer(self.0.ctx.ctx, self.0.as_js_value()) }
    }

    /// Reference to value
    #[inline]
    pub fn as_value(&self) -> &Value<'js> {
        self.0.as_value()
    }

    /// Convert into value
    #[inline]
    pub fn into_value(self) -> Value<'js> {
        self.0.into_value()
    }

    /// Convert from value
    pub fn from_value(value: Value<'js>) -> Result<Self> {
        Self::from_object(Object::from_value(value)?)
    }

    /// Reference as an object
    #[inline]
    pub fn as_object(&self) -> &Object<'js> {
        &self.0
    }

    /// Convert into an object
    #[inline]
    pub fn into_object(self) -> Object<'js> {
        self.0
    }

    /// Convert from an object
    pub fn from_object(object: Object<'js>) -> Result<Self> {
        if Self::get_raw(&object.0).is_some() {
            Ok(Self(object))
        } else {
            Err(Error::new_from_js("object", "ArrayBuffer"))
        }
    }

    pub(crate) fn get_raw(val: &Value<'js>) -> Option<(usize, *mut u8)> {
        let ctx = val.ctx;
        let val = val.as_js_value();
        let mut size = MaybeUninit::<qjs::size_t>::uninit();
        let ptr = unsafe { qjs::JS_GetArrayBuffer(ctx.ctx, size.as_mut_ptr(), val) };

        if ptr.is_null() {
            None
        } else {
            let len = unsafe { size.assume_init() } as _;
            Some((len, ptr))
        }
    }
}

impl<'js, T> AsRef<[T]> for ArrayBuffer<'js> {
    fn as_ref(&self) -> &[T] {
        let (len, ptr) = Self::get_raw(&self.0).expect("Not an ArrayBuffer");
        //assert!(len % size_of::<T>() == 0);
        let len = len / size_of::<T>();
        unsafe { slice::from_raw_parts(ptr as _, len) }
    }
}

impl<'js, T> AsMut<[T]> for ArrayBuffer<'js> {
    fn as_mut(&mut self) -> &mut [T] {
        let (len, ptr) = Self::get_raw(&self.0).expect("Not an ArrayBuffer");
        //assert!(len % size_of::<T>() == 0);
        let len = len / size_of::<T>();
        unsafe { slice::from_raw_parts_mut(ptr as _, len) }
    }
}

impl<'js> Deref for ArrayBuffer<'js> {
    type Target = Object<'js>;

    fn deref(&self) -> &Self::Target {
        self.as_object()
    }
}

impl<'js> AsRef<Object<'js>> for ArrayBuffer<'js> {
    fn as_ref(&self) -> &Object<'js> {
        self.as_object()
    }
}

impl<'js> AsRef<Value<'js>> for ArrayBuffer<'js> {
    fn as_ref(&self) -> &Value<'js> {
        self.as_value()
    }
}

impl<'js> FromJs<'js> for ArrayBuffer<'js> {
    fn from_js(_: Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Self::from_value(value)
    }
}

impl<'js> IntoJs<'js> for ArrayBuffer<'js> {
    fn into_js(self, _: Ctx<'js>) -> Result<Value<'js>> {
        Ok(self.into_value())
    }
}

#[cfg(test)]
mod test {
    use crate::*;

    #[test]
    fn from_javascript_i8() {
        test_with(|ctx| {
            let val: ArrayBuffer = ctx
                .eval(
                    r#"
                        new Int8Array([0, -5, 1, 11]).buffer
                    "#,
                )
                .unwrap();
            assert_eq!(val.len(), 4);
            assert_eq!(val.as_ref() as &[i8], &[0i8, -5, 1, 11]);
        });
    }

    #[test]
    fn into_javascript_i8() {
        test_with(|ctx| {
            let val = ArrayBuffer::new(ctx, [-1i8, 0, 22, 5]).unwrap();
            ctx.globals().set("a", val).unwrap();
            let res: i8 = ctx
                .eval(
                    r#"
                        let v = new Int8Array(a);
                        v.length != 4 ? 1 :
                        v[0] != -1 ? 2 :
                        v[1] != 0 ? 3 :
                        v[2] != 22 ? 4 :
                        v[3] != 5 ? 5 :
                        0
                    "#,
                )
                .unwrap();
            assert_eq!(res, 0);
        })
    }

    #[test]
    fn from_javascript_f32() {
        test_with(|ctx| {
            let val: ArrayBuffer = ctx
                .eval(
                    r#"
                        new Float32Array([0.5, -5.25, 123.125]).buffer
                    "#,
                )
                .unwrap();
            assert_eq!(val.len(), 12);
            assert_eq!(val.as_ref() as &[f32], &[0.5f32, -5.25, 123.125]);
        });
    }

    #[test]
    fn into_javascript_f32() {
        test_with(|ctx| {
            let val = ArrayBuffer::new(ctx, [-1.5f32, 0.0, 2.25]).unwrap();
            ctx.globals().set("a", val).unwrap();
            let res: i8 = ctx
                .eval(
                    r#"
                        let v = new Float32Array(a);
                        a.byteLength != 12 ? 1 :
                        v.length != 3 ? 2 :
                        v[0] != -1.5 ? 3 :
                        v[1] != 0 ? 4 :
                        v[2] != 2.25 ? 5 :
                        0
                    "#,
                )
                .unwrap();
            assert_eq!(res, 0);
        })
    }
}
