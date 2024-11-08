use crate::{qjs, Error, Result, Runtime};
#[cfg(feature = "quickjs-libc")]
use crate::{AsyncCtx, IntoJs, SendSyncJsValue};
use std::mem;

#[cfg(feature = "quickjs-libc")]
use std::{any::Any, sync::{Arc, atomic::AtomicI32}, ffi::CString};

mod builder;
pub use builder::{intrinsic, ContextBuilder, Intrinsic};
mod ctx;
pub use ctx::{Ctx, EvalOptions};
mod multi_with_impl;

/// A trait for using multiple contexts at the same time.
pub trait MultiWith<'js> {
    type Arg;

    /// Use multiple contexts together.
    ///
    /// # Panic
    /// This function will panic if any of the contexts are of seperate runtimes.
    fn with<R, F: FnOnce(Self::Arg) -> R>(self, f: F) -> R;
}

/// A single execution context with its own global variables and stack.
///
/// Can share objects with other contexts of the same runtime.
pub struct Context {
    //TODO replace with NotNull?
    pub(crate) ctx: *mut qjs::JSContext,
    rt: Runtime,
}

impl Clone for Context {
    fn clone(&self) -> Context {
        let ctx = unsafe { qjs::JS_DupContext(self.ctx) };
        let rt = self.rt.clone();
        Self { ctx, rt }
    }
}

impl Context {
    pub fn from_ctx<'js>(ctx: Ctx<'js>) -> Result<Self> {
        let rt = unsafe { &ctx.get_opaque().runtime }
            .try_ref()
            .ok_or(Error::Unknown)?;
        let ctx = unsafe { qjs::JS_DupContext(ctx.ctx) };
        Ok(Self { ctx, rt })
    }

    /// Creates a base context with only the required functions registered.
    /// If additional functions are required use [`Context::custom`],
    /// [`Context::builder`] or [`Context::full`].
    pub fn base(runtime: &Runtime) -> Result<Self> {
        Self::custom::<intrinsic::Base>(runtime)
    }

    /// Creates a context with only the required intrinsics registered.
    /// If additional functions are required use [`Context::custom`],
    /// [`Context::builder`] or [`Context::full`].
    pub fn custom<I: Intrinsic>(runtime: &Runtime) -> Result<Self> {
        let guard = runtime.inner.lock();
        let ctx = unsafe { qjs::JS_NewContextRaw(guard.rt) };
        if ctx.is_null() {
            return Err(Error::Allocation);
        }
        unsafe { I::add_intrinsic(ctx) };
        unsafe { Self::init_raw(ctx) };
        let res = Context {
            ctx,
            rt: runtime.clone(),
        };
        mem::drop(guard);

        Ok(res)
    }

    /// Creates a context with all standart available intrinsics registered.
    /// If precise controll is required of which functions are available use
    /// [`Context::custom`] or [`Context::builder`].
    pub fn full(runtime: &Runtime) -> Result<Self> {
        let guard = runtime.inner.lock();
        let ctx = unsafe { qjs::JS_NewContext(guard.rt) };
        if ctx.is_null() {
            return Err(Error::Allocation);
        }
        unsafe { Self::init_raw(ctx) };
        let res = Context {
            ctx,
            rt: runtime.clone(),
        };
        // Explicitly drop the guard to ensure it is valid during the entire use of runtime
        mem::drop(guard);

        Ok(res)
    }

    /// Creates a context with all standart available intrinsics registered 
    /// and quickjs-libc.
    #[cfg(feature = "quickjs-libc")]
    pub fn full_with_libc(runtime: &Runtime, args: Option<Vec<CString>>) -> Result<Self> {
        let guard = runtime.inner.lock();
        let ctx = unsafe { qjs::JS_NewCustomContext(guard.rt) };
        if ctx.is_null() {
            return Err(Error::Allocation);
        }
		let mut args = args.map(|args| args
			.into_iter()
			.map(|str| str.into_raw() as _)
			.collect::<Vec<*mut ::std::os::raw::c_char>>()
		);
		let (argc, argv) = if let Some(args) = args.as_mut() {
			(args.len() as _, args.as_mut_ptr())
		} else {
			(-1, std::ptr::null_mut())
		};
        unsafe { 
            crate::Function::init_raw(ctx);
            qjs::JS_AddIntrinsicWebAssembly(ctx);
            qjs::js_std_add_helpers(ctx, argc, argv);
        }
        let res = Context {
            ctx,
            rt: runtime.clone(),
        };
        // Explicitly drop the guard to ensure it is valid during the entire use of runtime
        mem::drop(guard);
		let _: Option<Vec<CString>> = args.map(|args| args
			.into_iter()
			.map(|ptr| unsafe { CString::from_raw(ptr) })
			.collect()
		);

        Ok(res)
    }

    /// Create a context builder for creating a context with a specific set of intrinsics
    pub fn builder() -> ContextBuilder<()> {
        ContextBuilder::default()
    }

    pub fn enable_big_num_ext(&self, enable: bool) {
        let guard = self.rt.inner.lock();
        guard.update_stack_top();
        unsafe { qjs::JS_EnableBignumExt(self.ctx, i32::from(enable)) }
        // Explicitly drop the guard to ensure it is valid during the entire use of runtime
        mem::drop(guard)
    }

    /// Returns the associated runtime
    pub fn runtime(&self) -> &Runtime {
        &self.rt
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn async_ctx_mut(&self) -> &mut AsyncCtx {
        use crate::runtime::Opaque;

        unsafe {
            let rt = qjs::JS_GetRuntime(self.ctx);
            let opaque: &mut Opaque = &mut *(qjs::JS_GetRustRuntimeOpaque(rt) as *mut _);
            opaque.get_async_ctx_mut()
        }
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn async_ctx(&self) -> &AsyncCtx {
        use crate::runtime::Opaque;

        unsafe {
            let rt = qjs::JS_GetRuntime(self.ctx);
            let opaque: &mut Opaque = &mut *(qjs::JS_GetRustRuntimeOpaque(rt) as *mut _);
            opaque.get_async_ctx()
        }
    }

    pub(crate) fn get_runtime_ptr(&self) -> *mut qjs::JSRuntime {
        unsafe { qjs::JS_GetRuntime(self.ctx) }
    }

    /// A entry point for manipulating and using javascript objects and scripts.
    /// The api is structured this way to avoid repeated locking the runtime when ever
    /// any function is called. This way the runtime is locked once before executing the callback.
    /// Furthermore, this way it is impossible to use values from different runtimes in this
    /// context which would otherwise be undefined behaviour.
    ///
    ///
    /// This is the only way to get a [`Ctx`] object.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Ctx) -> R,
    {
        #[cfg(any(not(feature = "futures"), feature = "quickjs-libc"))]
        {
            let guard = self.rt.inner.lock();
            guard.update_stack_top();
            let ctx = Ctx::new(self);
            let result = f(ctx);
            mem::drop(guard);
            result
        }

        #[cfg(all(feature = "futures", not(feature = "quickjs-libc")))]
        {
            let (spawn_pending_jobs, result) = {
                let guard = self.rt.inner.lock();
                guard.update_stack_top();
                let ctx = Ctx::new(self);
                let result = f(ctx);
                (guard.has_spawner() && guard.is_job_pending(), result)
            };
            if spawn_pending_jobs {
                self.rt.spawn_pending_jobs();
            }
            result
        }
    }

    pub unsafe fn init_raw(ctx: *mut qjs::JSContext) {
        crate::Function::init_raw(ctx);
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        //TODO
        let guard = match self.rt.inner.try_lock() {
            Some(x) => x,
            None => {
                let p = unsafe { &mut *(self.ctx as *const _ as *mut qjs::JSRefCountHeader) };
                if p.ref_count <= 1 {
                    // Lock was poisened, this should only happen on a panic.
                    // We should still free the context.
                    // TODO see if there is a way to recover from a panic which could cause the
                    // following assertion to trigger
                    assert!(std::thread::panicking());
                }
                unsafe { qjs::JS_FreeContext(self.ctx) }
                return;
            }
        };
        guard.update_stack_top();
        unsafe { qjs::JS_FreeContext(self.ctx) }
        // Explicitly drop the guard to ensure it is valid during the entire use of runtime
        mem::drop(guard);
    }
}

// Since the reference to runtime is behind a Arc this object is send
//
#[cfg(feature = "parallel")]
unsafe impl Send for Context {}

// Since all functions lock the global runtime lock access is synchronized so
// this object is sync
#[cfg(feature = "parallel")]
unsafe impl Sync for Context {}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct ContextWrapper(Arc<Context>);
// js context wrapped by ContextWrapper will not be dropped in threads other than js runtime thread
#[cfg(feature = "quickjs-libc")]
unsafe impl Send for ContextWrapper {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for ContextWrapper {}

#[cfg(feature = "quickjs-libc")]
impl ContextWrapper {
    pub fn new(context: Context) -> Self {
        Self(Arc::new(context))
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Ctx) -> R
    {
        self.0.with(f)
    }

    pub fn get_ctx<'js>(&'js self) -> Ctx<'js> {
        Ctx::new(&self.0)
    }

    pub fn runtime(&self) -> &Runtime {
        self.0.runtime()
    }

    pub fn async_ctx_mut(&self) -> &mut AsyncCtx {
        self.0.async_ctx_mut()
    }

    pub fn async_ctx(&self) -> &AsyncCtx {
        self.0.async_ctx()
    }

    pub fn ref_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct JsMessageCtx {
    ctx: SendSyncContext,
    async_ctx: AsyncCtx,
    resolve: SendSyncJsValue,
    reject: SendSyncJsValue,
    total: Arc<AtomicI32>,
}

#[cfg(feature = "quickjs-libc")]
impl JsMessageCtx {
    pub(crate) fn new(
        ctx: SendSyncContext, 
        async_ctx: AsyncCtx,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        total: Arc<AtomicI32>
    ) -> Self {
        Self {
            ctx,
            async_ctx,
            resolve,
            reject,
            total
        }
    }

    pub fn ctx(&self) -> Context {
        self.ctx.0.clone()
    }

    pub fn resolve<'js, T>(self, res: Box<T>) 
    where
        T: IntoJs<'js>
    {
        let context = self.ctx.into_inner();
        let ctx = Ctx::new(unsafe { std::mem::transmute(&context) });
        let resolve = self.resolve.into_inner(ctx).unwrap();
        let resolve = resolve.into_function().unwrap();
        let value = res.into_js(ctx).unwrap();
        resolve.call::<_, crate::Value>((value, )).unwrap();
        self.total.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn reject(self, err: crate::Error) {
        let context = self.ctx.into_inner();
        let ctx = Ctx::new(&context);
        let reject = self.reject.into_inner(ctx).unwrap();
        let reject = reject.into_function().unwrap();
        let value = err.into_js(ctx).unwrap();
        reject.call::<_, crate::Value>((value, )).unwrap();
        self.total.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn spawn_wasm_task(
        mut self, 
        func: impl FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static,
        promise_func: impl FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + 'static,
    ) {
        self.async_ctx.spawn_wasm_task(
            self.ctx, 
            Some(Box::new(func)), 
            Box::new(promise_func),
            self.resolve, 
            self.reject, 
            false
        );
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct WasmMessageCtx {
    ctx: SendSyncContext,
    async_ctx: AsyncCtx,
    resolve: SendSyncJsValue,
    reject: SendSyncJsValue,
}

// resolve and reject will not be called in wasm thread
#[cfg(feature = "quickjs-libc")]
unsafe impl Send for WasmMessageCtx {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for WasmMessageCtx {}

#[cfg(feature = "quickjs-libc")]
impl WasmMessageCtx {
    pub(crate) fn new(
        ctx: SendSyncContext,
        async_ctx: AsyncCtx,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
    ) -> Self {
        Self {
            ctx,
            async_ctx,
            resolve,
            reject,
        }
    }

    pub fn resolve<'js, T>(self, res: Box<T>) 
    where
        T: IntoJs<'js> + 'static,
    {
        self.spawn_js_task(None, move |ctx, _| ctx.resolve(res));
    }

    pub fn reject(self, err: crate::Error) {
        self.spawn_js_task(None, move |ctx, _| ctx.reject(err));
    }

    pub fn reject_with_clocure(self, err: crate::Error, func: Box<dyn FnOnce()>) {
        self.spawn_js_task(None, move |ctx, _| {
            func();
            ctx.reject(err)
        });
    }

    pub fn spawn_js_task(
        mut self, 
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: impl FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>) + 'static,
    ) {
        self.async_ctx.spawn_js_task(
            self.ctx, 
            func, 
            Box::new(promise_func),
            self.resolve, 
            self.reject, 
            false
        );
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct SendSyncContext(Context);

#[cfg(feature = "quickjs-libc")]
unsafe impl Send for SendSyncContext {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for SendSyncContext {}

#[cfg(feature = "quickjs-libc")]
impl SendSyncContext {
    pub fn new(ctx: Context) -> Self {
        Self(ctx)
    }

    pub fn into_inner(self) -> Context {
        self.0
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;

    #[test]
    fn base() {
        test_with(|ctx| {
            let val: Value = ctx.eval(r#"1+1"#).unwrap();

            assert_eq!(val.type_of(), Type::Int);
            assert_eq!(i32::from_js(ctx, val).unwrap(), 2);
            println!("{:?}", ctx.globals());
        });
    }

    #[test]
    fn minimal() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::builder()
            .with::<intrinsic::Eval>()
            .build(&rt)
            .unwrap();
        ctx.with(|ctx| {
            let val: i32 = ctx.eval(r#"1+1"#).unwrap();

            assert_eq!(val, 2);
            println!("{:?}", ctx.globals());
        });
    }

    #[cfg(feature = "exports")]
    #[test]
    fn module() {
        test_with(|ctx| {
            let _value: Module = ctx
                .compile(
                    "test_mod",
                    r#"
                    let t = "3";
                    let b = (a) => a + 3;
                    export { b, t}
                "#,
                )
                .unwrap();
        });
    }

    #[test]
    #[cfg(feature = "parallel")]
    fn parallel() {
        use std::thread;

        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            let _: () = ctx.eval("this.foo = 42").unwrap();
        });
        thread::spawn(move || {
            ctx.with(|ctx| {
                let i: i32 = ctx.eval("foo + 8").unwrap();
                assert_eq!(i, 50);
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    #[should_panic(
        expected = "Exception generated by quickjs: [eval_script]:1 invalid first character of private name\n    at eval_script:1\n"
    )]
    fn exception() {
        test_with(|ctx| {
            let val = ctx.eval::<(), _>("bla?#@!@ ");
            if let Err(e) = val {
                assert!(e.is_exception());
                panic!("{}", e);
            } else {
                panic!();
            }
        });
    }
}
