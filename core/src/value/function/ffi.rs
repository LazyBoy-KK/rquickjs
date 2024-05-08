use super::Input;
use crate::{handle_panic, qjs, ClassId, Ctx, Object, Result, Value};
#[cfg(feature = "quickjs-libc")]
use crate::RefsMarker;
use std::{ops::Deref, panic::AssertUnwindSafe, ptr};

#[cfg(feature = "quickjs-libc")]
use std::marker::PhantomData;

#[cfg(feature = "quickjs-libc")]
use crate::{ClassDef, Error, AsFunction};

static mut FUNC_CLASS_ID: ClassId = ClassId::new();

type BoxedFunc<'js> = Box<dyn Fn(&Input<'js>) -> Result<Value<'js>>>;

#[repr(transparent)]
pub struct JsFunction<'js>(BoxedFunc<'js>);

impl<'js> Deref for JsFunction<'js> {
    type Target = BoxedFunc<'js>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'js> JsFunction<'js> {
    pub fn new<F>(func: F) -> Self
    where
        F: Fn(&Input<'js>) -> Result<Value<'js>> + 'static,
    {
        Self(Box::new(func))
    }

    pub fn class_id() -> qjs::JSClassID {
        unsafe { &FUNC_CLASS_ID }.get() as _
    }

    pub unsafe fn into_js_value(self, ctx: Ctx<'_>) -> qjs::JSValue {
        let obj = qjs::JS_NewObjectClass(ctx.ctx, Self::class_id() as _);
        qjs::JS_SetOpaque(obj, Box::into_raw(Box::new(self)) as _);
        obj
    }

    pub unsafe fn into_error_js_value(self, ctx: Ctx<'_>) -> qjs::JSValue {
        let obj = if let Ok(error_proto) = ctx.eval::<Object, _>("Error") {
            qjs::JS_NewObjectProtoClass(
                ctx.ctx,
                error_proto.as_js_value() as _,
                Self::class_id() as _,
            )
        } else {
            qjs::JS_NewObjectClass(ctx.ctx, Self::class_id() as _)
        };
        qjs::JS_SetOpaque(obj, Box::into_raw(Box::new(self)) as _);
        obj
    }

    unsafe fn _call(
        &self,
        ctx: *mut qjs::JSContext,
        this: qjs::JSValue,
        argc: qjs::c_int,
        argv: *mut qjs::JSValue,
    ) -> Result<qjs::JSValue> {
        let input = Input::new_raw(ctx, this, argc, argv);

        let res = self.0(&input)?;

        Ok(res.into_js_value())
    }

    pub unsafe fn register(ctx: *mut qjs::JSContext) {
        FUNC_CLASS_ID.init();
        let class_id = Self::class_id();
        let rt = unsafe { qjs::JS_GetRuntime(ctx) };
        if 0 == qjs::JS_IsRegisteredClass(rt, class_id) {
            let class_def = qjs::JSClassDef {
                class_name: b"RustFunction\0".as_ptr() as *const _,
                finalizer: Some(Self::finalizer),
                gc_mark: None,
                call: Some(Self::call),
                exotic: ptr::null_mut(),
            };
            assert!(qjs::JS_NewClass(rt, class_id, &class_def) == 0);
            if let Ok(obj) = Ctx::from_ptr(ctx).eval::<Object, _>("Function") {
                if let Ok(func_proto) = obj.get_prototype() {
                    unsafe { qjs::JS_SetClassProto(ctx, class_id, func_proto.0.into_js_value()) }
                }
            }
        }
    }

    unsafe extern "C" fn call(
        ctx: *mut qjs::JSContext,
        func: qjs::JSValue,
        this: qjs::JSValue,
        argc: qjs::c_int,
        argv: *mut qjs::JSValue,
        _flags: qjs::c_int,
    ) -> qjs::JSValue {
        let is_ctor = qjs::JS_IsConstructor(ctx, func) as u32;
        let ctx = Ctx::from_ptr(ctx);
        let call_ctor = _flags as u32 & qjs::JS_CALL_FLAG_CONSTRUCTOR;
        if (is_ctor ^ call_ctor) == 1 {
            let error_str = String::from("must be called with new").into_bytes();
            let message = std::ffi::CString::from_vec_unchecked(error_str);
            return qjs::JS_ThrowTypeError(ctx.ctx, message.as_ptr());
        }

        let opaque = &*(qjs::JS_GetOpaque2(ctx.ctx, func, Self::class_id()) as *mut Self);

        handle_panic(
            ctx.ctx,
            AssertUnwindSafe(|| {
                opaque
                    ._call(ctx.ctx, this, argc, argv)
                    .unwrap_or_else(|error| error.throw(ctx))
            }),
        )
    }

    unsafe extern "C" fn finalizer(_rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
        let _opaque = Box::from_raw(qjs::JS_GetOpaque(val, Self::class_id()) as *mut Self);
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct JsFunctionWithClass<C, A, R> {
    class: C,
    _phantom: PhantomData<(A, R)>
}

#[cfg(feature = "quickjs-libc")]
impl<'js, C, A, R> JsFunctionWithClass<C, A, R>
where
    C: ClassDef + AsFunction<'js, A, R>,
{
    pub fn new(class: C) -> Self {
        Self {
            class,
            _phantom: PhantomData
        }
    }

    pub fn class_id() -> qjs::JSClassID {
        unsafe { C::class_id() }.get() as _
    }

    pub unsafe fn into_js_value(self, ctx: Ctx<'_>) -> qjs::JSValue {
        let obj = qjs::JS_NewObjectClass(ctx.ctx, Self::class_id() as _);
        qjs::JS_SetOpaque(obj, Box::into_raw(Box::new(self)) as _);
        obj
    }

    pub unsafe fn get_opaque(obj: qjs::JSValueConst) -> Result<&'js C> {
        let ptr = qjs::JS_GetOpaque(obj, Self::class_id()) as *const Self;
        if ptr.is_null() {
            return Err(Error::new_type_error(format!(
                "Not a {} object",
                C::CLASS_NAME
            )));
        }
        let opaque = &*ptr;
        Ok(&opaque.class)
    }

    unsafe fn _call(
        &self,
        ctx: *mut qjs::JSContext,
        this: qjs::JSValue,
        argc: qjs::c_int,
        argv: *mut qjs::JSValue,
    ) -> Result<qjs::JSValue> {
        let input = Input::new_raw(ctx, this, argc, argv);

        let res = self.class.call(&input)?;

        Ok(res.into_js_value())
    }

    pub fn register(ctx: Ctx) -> Result<()> {
        unsafe {
            let class_id = C::class_id();
            class_id.init();
            let class_id = Self::class_id();
            let rt = qjs::JS_GetRuntime(ctx.ctx);
            let class_name = std::ffi::CString::new(C::CLASS_NAME)?;
            if 0 == qjs::JS_IsRegisteredClass(rt, class_id) {
                let class_def = qjs::JSClassDef {
                    class_name: class_name.as_ptr(),
                    finalizer: Some(Self::finalizer),
                    gc_mark: if C::HAS_REFS {
                        Some(Self::gc_mark)
                    } else {
                        None
                    },
                    call: Some(Self::call),
                    exotic: ptr::null_mut(),
                };
                assert!(qjs::JS_NewClass(rt, class_id, &class_def) == 0);
                if let Ok(obj) = Ctx::from_ptr(ctx.ctx).eval::<Object, _>("Function") {
                    if let Ok(func_proto) = obj.get_prototype() {
                        qjs::JS_SetClassProto(ctx.ctx, class_id, func_proto.0.into_js_value())
                    }
                }
            }
        }
        Ok(())
    }

    unsafe extern "C" fn call(
        ctx: *mut qjs::JSContext,
        func: qjs::JSValue,
        this: qjs::JSValue,
        argc: qjs::c_int,
        argv: *mut qjs::JSValue,
        _flags: qjs::c_int,
    ) -> qjs::JSValue {
        let is_ctor = qjs::JS_IsConstructor(ctx, func) as u32;
        let ctx = Ctx::from_ptr(ctx);
        let call_ctor = _flags as u32 & qjs::JS_CALL_FLAG_CONSTRUCTOR;
        if (is_ctor ^ call_ctor) == 1 {
            let error_str = String::from("must be called with new").into_bytes();
            let message = std::ffi::CString::from_vec_unchecked(error_str);
            return qjs::JS_ThrowTypeError(ctx.ctx, message.as_ptr());
        }

        let opaque = &*(qjs::JS_GetOpaque2(ctx.ctx, func, Self::class_id()) as *mut Self);

        handle_panic(
            ctx.ctx,
            AssertUnwindSafe(|| {
                opaque
                    ._call(ctx.ctx, this, argc, argv)
                    .unwrap_or_else(|error| error.throw(ctx))
            }),
        )
    }

    unsafe extern "C" fn gc_mark(
        rt: *mut qjs::JSRuntime,
        val: qjs::JSValue,
        mark_func: qjs::JS_MarkFunc,
    ) {
        let ptr = qjs::JS_GetOpaque(val, Self::class_id()) as *mut Self;
        debug_assert!(!ptr.is_null());
        let opaque = &mut *ptr;
        let inst = &mut opaque.class;
        let marker = RefsMarker { rt, mark_func };
        inst.mark_refs(&marker);
    }

    unsafe extern "C" fn finalizer(_rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
        let _ = Box::from_raw(qjs::JS_GetOpaque(val, Self::class_id()) as *mut Self);
    }
}
