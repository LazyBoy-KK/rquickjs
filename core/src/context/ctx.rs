use crate::{
    handle_exception, markers::Invariant, qjs, runtime::Opaque, Context, FromJs, Function, Module,
    Object, Result, Value,
};

#[cfg(all(feature = "futures", not(feature = "quickjs-libc")))]
use std::future::Future;

#[cfg(all(feature = "futures", not(feature = "quickjs-libc")))]
use crate::ParallelSend;

#[cfg(feature = "quickjs-libc")]
use crate::runtime::AsyncCtx;

#[cfg(feature = "registery")]
use crate::RegisteryKey;

use std::{
    ffi::{CStr, CString},
    fs,
    marker::PhantomData,
    mem,
    path::Path,
};

/// Eval options.
#[derive(Clone)]
pub struct EvalOptions {
    /// Global code.
    pub global: bool,
    /// Force 'strict' mode.
    pub strict: bool,
    /// Don't include the stack frames before this eval in the Error() backtraces.
    pub backtrace_barrier: bool,
}

impl EvalOptions {
    fn to_flag(&self) -> i32 {
        let mut flag = if self.global {
            qjs::JS_EVAL_TYPE_GLOBAL
        } else {
            qjs::JS_EVAL_TYPE_MODULE
        };

        if self.strict {
            flag |= qjs::JS_EVAL_FLAG_STRICT;
        }

        if self.backtrace_barrier {
            flag |= qjs::JS_EVAL_FLAG_BACKTRACE_BARRIER;
        }

        flag as i32
    }
}

impl Default for EvalOptions {
    fn default() -> Self {
        EvalOptions {
            global: true,
            strict: true,
            backtrace_barrier: false,
        }
    }
}

/// Context in use, passed to [`Context::with`].
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'js> {
    pub(crate) ctx: *mut qjs::JSContext,
    marker: Invariant<'js>,
}

impl<'js> Ctx<'js> {
    pub fn from_ptr(ctx: *mut qjs::JSContext) -> Self {
        Ctx {
            ctx,
            marker: PhantomData,
        }
    }

    pub fn new(ctx: &'js Context) -> Self {
        Ctx {
            ctx: ctx.ctx,
            marker: PhantomData,
        }
    }

    pub(crate) unsafe fn eval_raw<S: Into<Vec<u8>>>(
        self,
        source: S,
        file_name: &CStr,
        flag: i32,
    ) -> Result<qjs::JSValue> {
        let src = source.into();
        let len = src.len();
        let src = CString::new(src)?;
        let val = qjs::JS_Eval(self.ctx, src.as_ptr(), len as _, file_name.as_ptr(), flag);
        handle_exception(self, val)
    }

    /// Evaluate a script in global context.
    pub fn eval<V: FromJs<'js>, S: Into<Vec<u8>>>(self, source: S) -> Result<V> {
        self.eval_with_options(source, Default::default())
    }

    /// Evaluate a script with the given options.
    pub fn eval_with_options<V: FromJs<'js>, S: Into<Vec<u8>>>(
        self,
        source: S,
        options: EvalOptions,
    ) -> Result<V> {
        let file_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"eval_script\0") };

        V::from_js(self, unsafe {
            let val = self.eval_raw(source, file_name, options.to_flag())?;
            Value::from_js_value(self, val)
        })
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn eval_with_options_and_std_loop<V: FromJs<'js>, S: Into<Vec<u8>>>(
        self,
        source: S,
        options: EvalOptions,
    ) -> Result<V> {
        let file_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"eval_script\0") };

        V::from_js(self, unsafe {
            let val = self.eval_raw(source, file_name, options.to_flag())?;
            qjs::js_std_loop(self.ctx);
            Value::from_js_value(self, val)
        })
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn eval_with_options_and_std_loop_test<V: FromJs<'js>, S: Into<Vec<u8>>>(
        self,
        source: S,
        options: EvalOptions
    ) -> Result<V> {
        let file_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"eval_script\0") };

        V::from_js(self, unsafe {
            let val = self.eval_raw(source, file_name, options.to_flag())?;
            qjs::js_std_loop_test(self.ctx);
            Value::from_js_value(self, val)
        })
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn eval_file_with_options_and_std_loop<V: FromJs<'js>, P: AsRef<Path>>(
        self,
        path: P,
        options: EvalOptions,
    ) -> Result<V> {
        let buffer = fs::read(path.as_ref())?;
        let file_name = CString::new(
            path.as_ref()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )?;

        unsafe {
            let val = self.eval_raw(buffer, file_name.as_c_str(), options.to_flag())?;
            qjs::js_std_loop(self.ctx);
            V::from_js(self, Value::from_js_value(self, val))
        }
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn eval_file_with_options_and_std_loop_test<V: FromJs<'js>, P: AsRef<Path>>(
        self,
        path: P,
        options: EvalOptions
    ) -> Result<V> {
        let buffer = fs::read(path.as_ref())?;
        let file_name = CString::new(
            path.as_ref()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )?;

        V::from_js(self, unsafe {
            let val = self.eval_raw(buffer, file_name.as_c_str(), options.to_flag())?;
            qjs::js_std_loop_test(self.ctx);
            Value::from_js_value(self, val)
        })
    }

    /// Evaluate a script directly from a file.
    pub fn eval_file<V: FromJs<'js>, P: AsRef<Path>>(self, path: P) -> Result<V> {
        self.eval_file_with_options(path, Default::default())
    }

    pub fn eval_file_with_options<V: FromJs<'js>, P: AsRef<Path>>(
        self,
        path: P,
        options: EvalOptions,
    ) -> Result<V> {
        let buffer = fs::read(path.as_ref())?;
        let file_name = CString::new(
            path.as_ref()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )?;

        V::from_js(self, unsafe {
            let val = self.eval_raw(buffer, file_name.as_c_str(), options.to_flag())?;
            Value::from_js_value(self, val)
        })
    }

    /// Compile a module for later use.
    pub fn compile<N, S>(self, name: N, source: S) -> Result<Module<'js>>
    where
        N: Into<Vec<u8>>,
        S: Into<Vec<u8>>,
    {
        let module = Module::new(self, name, source)?;
        module.eval()
    }

    /// Returns the global object of this context.
    pub fn globals(self) -> Object<'js> {
        unsafe {
            let v = qjs::JS_GetGlobalObject(self.ctx);
            Object::from_js_value(self, v)
        }
    }

    /// Creates promise and resolving functions.
    pub fn promise(self) -> Result<(Object<'js>, Function<'js>, Function<'js>)> {
        let mut funcs = mem::MaybeUninit::<[qjs::JSValue; 2]>::uninit();

        Ok(unsafe {
            let promise = handle_exception(
                self,
                qjs::JS_NewPromiseCapability(self.ctx, funcs.as_mut_ptr() as _),
            )?;
            let [resolve, reject] = funcs.assume_init();
            (
                Object::from_js_value(self, promise),
                Function::from_js_value(self, resolve),
                Function::from_js_value(self, reject),
            )
        })
    }

    #[cfg(feature = "quickjs-libc")]
    pub(crate) unsafe fn get_opaque(self) -> &'js mut Opaque {
        let rt = qjs::JS_GetRuntime(self.ctx);
        &mut *(qjs::JS_GetRustRuntimeOpaque(rt) as *mut _)
    }

	#[cfg(all(feature = "quickjs-libc", not(feature = "allocator"), not(feature = "quickjs-libc-test")))]
	pub fn inc_malloc_size(&self, size: usize) {
		unsafe {
			let rt = qjs::JS_GetRuntime(self.ctx);
			qjs::JS_IncMallocSize(rt, size as _);
		}
	}

	#[cfg(all(feature = "quickjs-libc", not(feature = "allocator"), not(feature = "quickjs-libc-test")))]
	pub fn dec_malloc_size(&self, size: usize) {
		unsafe {
			let rt = qjs::JS_GetRuntime(self.ctx);
			qjs::JS_DecMallocSize(rt, size as _);
		}
	}

    #[cfg(not(feature = "quickjs-libc"))]
    pub(crate) unsafe fn get_opaque(self) -> &'js mut Opaque {
        let rt = qjs::JS_GetRuntime(self.ctx);
        &mut *(qjs::JS_GetRuntimeOpaque(rt) as *mut _)
    }

    /// Spawn future using configured async runtime
    #[cfg(not(feature = "quickjs-libc"))]
    #[cfg(feature = "futures")]
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    pub fn spawn<F, T>(&self, future: F)
    where
        F: Future<Output = T> + ParallelSend + 'static,
        T: ParallelSend + 'static,
    {
        let opaque = unsafe { self.get_opaque() };
        opaque.get_spawner().spawn(future);
    }

    // #[cfg(feature = "quickjs-libc")]
    // #[cfg(feature = "futures")]
    // pub fn spawn<F>(&self, future: F)
    // where
    //     F: Future + 'static,
    //     F::Output: Send + 'static,
    // {
    //     let opaque = unsafe { self.get_opaque() };
    //     let async_ctx = opaque.get_async_ctx();
    //     let task = async_ctx.spawn_js_task(future);
    //     task.detach();
    // }

    #[cfg(feature = "quickjs-libc")]
    pub fn async_ctx_mut(&self) -> &mut AsyncCtx {
        let opaque = unsafe { self.get_opaque() };
        opaque.get_async_ctx_mut()
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn async_ctx(&self) -> &AsyncCtx {
        let opaque = unsafe { self.get_opaque() };
        opaque.get_async_ctx()
    }
}

#[cfg(feature = "registery")]
impl<'js> Ctx<'js> {
    /// Store a value in the registery so references to it can be kept outside the scope of context use.
    ///
    /// A registered value can be retrieved from any context belonging to the same runtime.
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "registery")))]
    pub fn register(self, v: Value<'js>) -> RegisteryKey {
        unsafe {
            let register = self.get_opaque();
            let key = RegisteryKey(v.into_js_value());
            register.registery.insert(key);
            key
        }
    }

    /// Remove a value from the registery.
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "registery")))]
    pub fn deregister(self, k: RegisteryKey) -> Option<Value<'js>> {
        unsafe {
            let register = self.get_opaque();
            if register.registery.remove(&k) {
                Some(Value::from_js_value(self, k.0))
            } else {
                None
            }
        }
    }

    /// Get a value from the registery.
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "registery")))]
    pub fn get_register(self, k: RegisteryKey) -> Option<Value<'js>> {
        unsafe {
            let opaque = self.get_opaque();
            if opaque.registery.contains(&k) {
                Some(Value::from_js_value_const(self, k.0))
            } else {
                None
            }
        }
    }
}

mod test {
    #[cfg(feature = "exports")]
    #[test]
    fn exports() {
        use crate::{intrinsic, Context, Function, Runtime};

        let runtime = Runtime::new().unwrap();
        let ctx = Context::custom::<(intrinsic::Promise, intrinsic::Eval)>(&runtime).unwrap();
        ctx.with(|ctx| {
            let module = ctx
                .compile("test", "export default async () => 1;")
                .unwrap();
            let func: Function = module.get("default").unwrap();
            func.call::<(), ()>(()).unwrap();
        });
    }

    #[test]
    fn eval() {
        use crate::{Context, Runtime};

        let runtime = Runtime::new().unwrap();
        let ctx = Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let res: String = ctx
                .eval(
                    r#"
                    function test() {
                        var foo = "bar";
                        return foo;
                    }

                    test()
                "#,
                )
                .unwrap();

            assert_eq!("bar".to_string(), res);
        })
    }

    #[test]
    #[should_panic(expected = "'foo' is not defined")]
    fn eval_with_sloppy_code() {
        use crate::{Context, Runtime};

        let runtime = Runtime::new().unwrap();
        let ctx = Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let _: String = ctx
                .eval(
                    r#"
                    function test() {
                        foo = "bar";
                        return foo;
                    }

                    test()
                "#,
                )
                .unwrap();
        })
    }

    #[test]
    fn eval_with_options_no_strict_sloppy_code() {
        use crate::{Context, EvalOptions, Runtime};

        let runtime = Runtime::new().unwrap();
        let ctx = Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let res: String = ctx
                .eval_with_options(
                    r#"
                    function test() {
                        foo = "bar";
                        return foo;
                    }

                    test()
                "#,
                    EvalOptions {
                        strict: false,
                        ..Default::default()
                    },
                )
                .unwrap();

            assert_eq!("bar".to_string(), res);
        })
    }

    #[test]
    #[should_panic(expected = "'foo' is not defined")]
    fn eval_with_options_strict_sloppy_code() {
        use crate::{Context, EvalOptions, Runtime};

        let runtime = Runtime::new().unwrap();
        let ctx = Context::full(&runtime).unwrap();
        ctx.with(|ctx| {
            let _: String = ctx
                .eval_with_options(
                    r#"
                    function test() {
                        foo = "bar";
                        return foo;
                    }

                    test()
                "#,
                    EvalOptions {
                        strict: true,
                        ..Default::default()
                    },
                )
                .unwrap();
        })
    }
}
