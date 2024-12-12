#[cfg(not(feature = "quickjs-libc"))]
use crate::{ParallelSend, Ref};
#[cfg(feature = "quickjs-libc")]
use crate::{SendSyncContext, SendSyncJsValue, WasmMessageCtx, JsMessageCtx, Result};

#[cfg(not(feature = "quickjs-libc"))]
use async_task::Runnable;

#[cfg(not(feature = "quickjs-libc"))]
use flume::{r#async::RecvStream, unbounded, Receiver, Sender};

#[cfg(not(feature = "quickjs-libc"))]
use futures_lite::Stream;

#[cfg(feature = "quickjs-libc")]
use crate::qjs;

#[cfg(not(feature = "quickjs-libc"))]
use std::{pin::Pin, task::{Waker, Context, Poll}, future::Future};

#[cfg(not(feature = "quickjs-libc"))]
use pin_project_lite::pin_project;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "quickjs-libc")]
use std::sync::Mutex;

#[cfg(feature = "quickjs-libc")]
use std::{
    any::Any, 
    sync::{atomic::AtomicI32, Arc},
    collections::VecDeque,
    cell::UnsafeCell
};

#[cfg(not(feature = "quickjs-libc"))]
#[cfg(feature = "parallel")]
use async_task::spawn as spawn_task;
#[cfg(not(feature = "quickjs-libc"))]
#[cfg(not(feature = "parallel"))]
use async_task::spawn_local as spawn_task;

#[cfg(not(feature = "quickjs-libc"))]
pin_project! {
    /// The async executor future
    ///
    /// The executor which returning by [`Runtime::run_executor`](crate::Runtime::run_executor).
    /// It should be spawned using preferred async runtime to get async features works as expected.
    /// The executor future will be pending until runtime is dropped.
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    pub struct Executor {
        #[pin]
        tasks: RecvStream<'static, Runnable>,
        idles: Receiver<Waker>,
        idle: Ref<AtomicBool>,
    }
}

#[cfg(not(feature = "quickjs-libc"))]
impl Executor {
    pub(crate) fn new() -> (Self, Spawner) {
        let (tasks_tx, tasks_rx) = unbounded();
        let (idles_tx, idles_rx) = unbounded();
        let idle = Ref::new(AtomicBool::new(true));
        (
            Self {
                tasks: tasks_rx.into_stream(),
                idles: idles_rx,
                idle: idle.clone(),
            },
            Spawner {
                tasks: tasks_tx,
                idles: idles_tx,
                idle,
            },
        )
    }
}

#[cfg(not(feature = "quickjs-libc"))]
impl Future for Executor {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let result = {
            if let Poll::Ready(task) = self.as_mut().project().tasks.poll_next(cx) {
                if let Some(task) = task {
                    task.run();
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                } else {
                    // spawner is closed and queue is empty
                    Poll::Ready(())
                }
            } else {
                // spawner is alive and queue is empty
                Poll::Pending
            }
        };

        self.idle.store(true, Ordering::SeqCst);

        // wake idle futures
        while let Ok(waker) = self.idles.try_recv() {
            waker.wake();
        }

        result
    }
}

#[cfg(not(feature = "quickjs-libc"))]
pub struct Spawner {
    tasks: Sender<Runnable>,
    idles: Sender<Waker>,
    idle: Ref<AtomicBool>,
}

#[cfg(not(feature = "quickjs-libc"))]
impl Spawner {
    pub fn spawn<F>(&self, future: F)
    where
        F: Future + ParallelSend + 'static,
    {
        self.idle.store(false, Ordering::SeqCst);
        let (runnable, task) = spawn_task(
            async move {
                future.await;
            },
            self.schedule(),
        );
        task.detach();
        runnable.schedule();
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let tasks = self.tasks.clone();
        move |runnable: Runnable| {
            tasks
                .send(runnable)
                .expect("Async executor unfortunately destroyed");
        }
    }

    pub fn idle(&self) -> Idle {
        if self.idle.load(Ordering::SeqCst) {
            Idle::default()
        } else {
            Idle::new(&self.idle, &self.idles)
        }
    }
}

#[cfg(not(feature = "quickjs-libc"))]
struct InnerIdle {
    idle: Ref<AtomicBool>,
    signal: Sender<Waker>,
}

#[cfg(not(feature = "quickjs-libc"))]
/// The idle awaiting future
#[derive(Default)]
pub struct Idle(Option<InnerIdle>);

#[cfg(not(feature = "quickjs-libc"))]
impl Idle {
    fn new(idle: &Ref<AtomicBool>, sender: &Sender<Waker>) -> Self {
        Self(Some(InnerIdle {
            idle: idle.clone(),
            signal: sender.clone(),
        }))
    }
}

#[cfg(not(feature = "quickjs-libc"))]
impl Future for Idle {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if let Some(inner) = &self.0 {
            if !inner.idle.load(Ordering::SeqCst) && inner.signal.send(cx.waker().clone()).is_ok() {
                return Poll::Pending;
            }
        }
        Poll::Ready(())
    }
}

#[cfg(feature = "quickjs-libc")]
#[derive(Debug)]
struct RustMsgPipe(*mut qjs::JSRustMessagePipe);

#[cfg(feature = "quickjs-libc")]
unsafe impl Send for RustMsgPipe {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for RustMsgPipe {}

#[cfg(feature = "quickjs-libc")]
impl Clone for RustMsgPipe {
    fn clone(&self) -> Self {
        let pipe = unsafe { qjs::JS_DupRustMessagePipe(self.0) };
        Self(pipe)
    }
}

#[cfg(feature = "quickjs-libc")]
impl Drop for RustMsgPipe {
    fn drop(&mut self) {
        unsafe { qjs::JS_FreeRustMessagePipe(self.0) }
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct WasmFuncMessage {
    ctx: WasmMessageCtx,
    func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
    promise_func: Box<dyn FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + Send>
}

#[cfg(feature = "quickjs-libc")]
impl WasmFuncMessage {
    pub(crate) fn call(self) {
        let res = self.func.map(|func| func());
        (self.promise_func)(self.ctx, res);
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct JsFuncMessage {
    ctx: JsMessageCtx,
    func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
    promise_func: Box<dyn FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>)>
}

#[cfg(feature = "quickjs-libc")]
impl JsFuncMessage {
    pub(crate) fn call(self) {
        let ctx = self.ctx;
        let res = self.func.map(|func| func(&ctx));
        (self.promise_func)(ctx, res);
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadRustTaskExecutor {
    wasm_tasks: WasmReceiver,
}

#[cfg(feature = "quickjs-libc")]
unsafe impl Send for ThreadRustTaskExecutor {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for ThreadRustTaskExecutor {}

#[cfg(feature = "quickjs-libc")]
impl ThreadRustTaskExecutor {
    pub fn run(&mut self) {
        loop {
            match self.wasm_tasks.recv() {
                Ok(msg) => msg.call(),
                Err(crossbeam_channel::RecvError) => break,
            }
        }
    }
}

#[cfg(feature = "quickjs-libc")]
struct ImportJsResInner {
	res: UnsafeCell<Result<()>>,
	flag: AtomicBool
}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct ImportJsFuncRes(Arc<ImportJsResInner>);

#[cfg(feature = "quickjs-libc")]
impl ImportJsFuncRes {
	pub(crate) fn set(&self, res: Result<()>) {
		unsafe {
			*self.0.res.get() = res;
		}
		self.0.flag.store(true, Ordering::Release);
	}

	pub fn get(self) -> Option<Result<()>> {
		if self.0.flag.load(Ordering::Acquire) {
			Arc::into_inner(self.0).map(|e| e.res.into_inner())
		} else {
			None
		}
	}
}

#[cfg(feature = "quickjs-libc")]
pub struct ImportJsFuncMessage {
	func: Box<dyn FnOnce() -> Result<()>>,
	thread: std::thread::Thread,
	result: ImportJsFuncRes,
}

#[cfg(feature = "quickjs-libc")]
impl ImportJsFuncMessage {
	pub(crate) fn new<F: FnOnce() -> Result<()> + 'static>(func: F) -> Self {
		Self {
			func: Box::new(func),
			thread: std::thread::current(),
			result: ImportJsFuncRes(Arc::new(ImportJsResInner { 
				res: UnsafeCell::new(Ok(())), 
				flag: AtomicBool::new(false) 
			})),
		}
	}

	pub(crate) fn result(&self) -> ImportJsFuncRes {
		self.result.clone()
	}

	pub(crate) fn call(self) {
		let res = (self.func)();
		self.result.set(res);
		self.thread.unpark();
	}
}

#[cfg(feature = "quickjs-libc")]
pub type WasmSender = crossbeam_channel::Sender<WasmFuncMessage>;

#[cfg(feature = "quickjs-libc")]
pub type WasmReceiver = crossbeam_channel::Receiver<WasmFuncMessage>;

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct ThreadCtx {
	sender: WasmSender,
	main_thread_id: std::thread::ThreadId
}

#[cfg(feature = "quickjs-libc")]
impl ThreadCtx {
	pub fn new(ctx: crate::Ctx) -> Self {
		let sender = ctx.async_ctx().create_thread();
		Self {
			sender,
			main_thread_id: ctx.async_ctx().get_main_thread_id()
		}
	}

	pub fn is_main_thread(&self) -> bool {
		std::thread::current().id() == self.main_thread_id
	}

	pub fn spawn_wasm_task(
		&self,
		ctx: crate::Ctx,
		send_sync_ctx: crate::SendSyncContext, 
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: impl FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + Send + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
	) {
		ctx.async_ctx_mut().spawn_wasm_task(
			send_sync_ctx, 
			self.sender.clone(), 
			func, 
			promise_func, 
			resolve, 
			reject, 
			new_task
		);
	}
}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct AsyncCtx(Arc<UnsafeCell<AsyncCtxInner>>);

#[cfg(feature = "quickjs-libc")]
impl crate::DeepSizeOf for AsyncCtx {
	fn deep_size_of_children(&self, context: &mut deepsize::Context) -> usize {
		self.inner().deep_size_of_children(context)
	}
}

#[cfg(feature = "quickjs-libc")]
impl AsyncCtx {
	fn inner(&self) -> &mut AsyncCtxInner {
		unsafe { &mut *self.0.get() }
	}

	pub(crate) fn new(rt: *mut qjs::JSRuntime) -> Self {
        Self(Arc::new(UnsafeCell::new(AsyncCtxInner::new(rt))))
    }

    pub fn spawn_wasm_task(
        &mut self, 
        ctx: crate::SendSyncContext, 
		sender: WasmSender,
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: impl FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + Send + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
        self.inner().spawn_wasm_task(ctx, self.clone(), sender, func, promise_func, resolve, reject, new_task);
    }

    pub fn spawn_js_task(
        &mut self, 
        ctx: crate::SendSyncContext, 
		sender: WasmSender,
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: impl FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>) + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
    ) {
        self.inner().spawn_js_task(ctx, self.clone(), sender, func, promise_func, resolve, reject);
    }

    pub fn spawn_import_js_task<F>(&self, func: F) -> ImportJsFuncRes
    where
        F: FnOnce() -> Result<()> + 'static
    {
        self.inner().spawn_import_js_task(func)
    }

	pub(crate) fn run_js_single_task(&self) -> i32 {
        self.inner().run_js_single_task()
    }

	// is thread that init async_ctx
    pub fn is_main_thread(&self) -> bool {
        self.inner().is_main_thread()
    }

	pub fn get_main_thread_id(&self) -> std::thread::ThreadId {
		self.inner().get_main_thread_id()
	}

	pub fn create_thread(&self) -> WasmSender {
		self.inner().create_thread()
	}
}

#[cfg(feature = "quickjs-libc")]
pub struct AsyncCtxInner {
	rt: *mut qjs::JSRuntime,
	pub spawner: ThreadTaskSpawner,
	js_exec: ThreadJsTaskExecutor,
	main_thread_id: std::thread::ThreadId,
}

#[cfg(feature = "quickjs-libc")]
impl crate::DeepSizeOf for AsyncCtxInner {
	fn deep_size_of_children(&self, _context: &mut deepsize::Context) -> usize {
		size_of::<VecDeque<JsFuncMessage>>() + size_of::<Vec<ImportJsFuncMessage>>() + size_of::<AtomicI32>() + size_of::<Mutex<bool>>()
	}
}

#[cfg(feature = "quickjs-libc")]
impl AsyncCtxInner {
    pub(crate) fn new(rt: *mut qjs::JSRuntime) -> Self {
        let js_task_tx = Arc::new(UnsafeCell::new(VecDeque::with_capacity(100)));
        let js_task_rx = js_task_tx.clone();
        let import_js_task_tx = Arc::new(UnsafeCell::new(VecDeque::with_capacity(1)));
        let import_js_task_rx = import_js_task_tx.clone();
		let total = TotalRefCount::new();

        let mutex = Arc::new(std::sync::Mutex::new(false));
        
        let spawner = ThreadTaskSpawner {
            pipe: None,
            js_tasks: js_task_tx,
            import_js_task: import_js_task_tx,
            mutex: mutex.clone(),
			total: total.clone(),
        };

        let js_exec = ThreadJsTaskExecutor {
            pipe: None,
            js_tasks: js_task_rx,
            import_js_task: import_js_task_rx,
            total,
            mutex
        };

        Self {
            rt,
            spawner,
            js_exec,
            main_thread_id: std::thread::current().id(),
        }
    }

	pub fn is_main_thread(&self) -> bool {
		std::thread::current().id() == self.main_thread_id
	}

	pub fn get_main_thread_id(&self) -> std::thread::ThreadId {
		self.main_thread_id
	}

	pub fn create_thread(&self) -> WasmSender {
		let (sender, receiver) = crossbeam_channel::unbounded();
		let mut executor = ThreadRustTaskExecutor {
			wasm_tasks: receiver
		};
		#[cfg(all(feature = "quickjs-libc", feature = "allocator", not(feature = "quickjs-libc-test")))]
		let is_main_runtime = crate::allocator::is_main_runtime();
        std::thread::spawn(move || {
			#[cfg(all(feature = "quickjs-libc", feature = "allocator", not(feature = "quickjs-libc-test")))]
			crate::allocator::set_is_main_runtime(is_main_runtime);
            executor.run();
        });
		sender
	}

    // init will be called only once
    fn init(&mut self) {
        let pipe = unsafe { qjs::JS_CreateRustMessagePipe(self.rt) };
        if pipe.is_null() {
            panic!("failed creating rust message pipe");
        }
		let pipe = RustMsgPipe(pipe);
        self.spawner.pipe = Some(pipe.clone());
        self.js_exec.pipe = Some(pipe);
    }

    pub fn spawn_wasm_task(
        &mut self, 
        ctx: crate::SendSyncContext, 
		async_ctx: AsyncCtx,
		sender: WasmSender,
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: impl FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + 'static + Send,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
		// 仅在第一次生成wasm任务时创建pipe
		// 若js代码未生成wasm任务则不得创建pipe
		if self.spawner.pipe.is_none() {
			self.init();
		}
        self.spawner.spawn_wasm_task(
            ctx,
            async_ctx,
			sender,
            func, 
            Box::new(promise_func),
            resolve, 
            reject, 
			new_task,
        );
    }

    pub fn spawn_js_task(
        &mut self, 
        ctx: crate::SendSyncContext, 
		async_ctx: AsyncCtx,
		sender: WasmSender,
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: impl FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>) + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
    ) {
        self.spawner.spawn_js_task(
            ctx, 
            async_ctx,
			sender,
            func, 
            Box::new(promise_func),
            resolve, 
            reject, 
        );
    }

    pub fn spawn_import_js_task<F>(&self, future: F) -> ImportJsFuncRes
    where
        F: FnOnce() -> Result<()> + 'static
    {
        self.spawner.spawn_import_js_task(future)
    }

    pub(crate) fn run_js_single_task(&mut self) -> i32 {
        self.js_exec.run()
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadTaskSpawner {
    pipe: Option<RustMsgPipe>,
    js_tasks: Arc<UnsafeCell<VecDeque<JsFuncMessage>>>,
    import_js_task: Arc<UnsafeCell<VecDeque<ImportJsFuncMessage>>>,
    mutex: Arc<std::sync::Mutex<bool>>,
	total: TotalRefCount,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadTaskSpawner {
    pub fn spawn_wasm_task(
        &mut self, 
        ctx: SendSyncContext, 
        async_ctx: AsyncCtx,
		sender: WasmSender,
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: Box<dyn FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + Send>,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
		new_task: bool,
    ) {
        let msg = WasmFuncMessage {
            ctx: WasmMessageCtx::new(ctx, async_ctx, sender.clone(), resolve, reject),
            func,
            promise_func,
        };
		if new_task {
			self.total.inc();
		}
		sender.send(msg).expect("send wasm msg failed");
    }

    pub fn spawn_js_task(
        &self, 
        ctx: SendSyncContext, 
        async_ctx: AsyncCtx,
		sender: WasmSender,
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: Box<dyn FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>)>,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
    ) {
        let msg = JsFuncMessage {
            ctx: JsMessageCtx::new(ctx, async_ctx, sender, resolve, reject, self.total.clone()),
            func,
            promise_func
        };
        let guard = self.mutex.lock().unwrap();
        if self.get_js_task_mut().is_empty() && self.get_import_js_task_mut().is_empty() {
            self.write_js_pipe();
        }
        self.get_js_task_mut().push_back(msg);
        drop(guard);
    }

    pub fn spawn_import_js_task<F>(&self, func: F) -> ImportJsFuncRes
    where
        F: FnOnce() -> Result<()> + 'static
    {
		let task = ImportJsFuncMessage::new(func);
		let res = task.result();

        let guard = self.mutex.lock().unwrap();
        if self.get_js_task_mut().is_empty() && self.get_import_js_task_mut().is_empty() {
            self.write_js_pipe();
        }
        self.get_import_js_task_mut().push_back(task);
        drop(guard);

		// waiting for result
		std::thread::park();
        res
    }

    pub fn write_js_pipe(&self) {
        let pipe = self.pipe.as_ref().unwrap();
		unsafe { qjs::JS_WriteRustMessagePipe(pipe.0); }
    }

    fn get_js_task_mut(&self) -> &mut VecDeque<JsFuncMessage> {
        unsafe { &mut *self.js_tasks.get() }
    }

    fn get_import_js_task_mut(&self) -> &mut VecDeque<ImportJsFuncMessage> {
        unsafe { &mut *self.import_js_task.get() }
    }
}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
/// Only used in main thread
pub struct TotalRefCount(Arc<UnsafeCell<u32>>);

#[cfg(feature = "quickjs-libc")]
impl TotalRefCount {
	pub fn new() -> Self {
		Self(Arc::new(UnsafeCell::new(0)))
	}

	fn inner(&self) -> &mut u32 {
		unsafe { &mut *self.0.get() }
	}

	#[inline]
	pub fn inc(&self) {
		*self.inner() += 1
	}

	#[inline]
	pub fn dec(&self) {
		*self.inner() -= 1
	}

	#[inline]
	pub fn get(&self) -> u32 {
		*self.inner()
	}
}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadJsTaskExecutor {
    pipe: Option<RustMsgPipe>,
    js_tasks: Arc<UnsafeCell<VecDeque<JsFuncMessage>>>,
    import_js_task: Arc<UnsafeCell<VecDeque<ImportJsFuncMessage>>>,
    total: TotalRefCount,
    mutex: Arc<std::sync::Mutex<bool>>,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadJsTaskExecutor {
    // 1 means completed, 0 means not completed
    pub fn run(&mut self) -> i32 {
        let pipe = self.pipe.as_ref().unwrap();
        let import_js_task = self.get_import_js_task_mut();
        let js_task = self.get_js_task_mut();

        let guard = self.mutex.lock().unwrap();
        if import_js_task.is_empty() && js_task.is_empty() {
            unsafe { 
                qjs::JS_ReadRustMessagePipe(pipe.0); 
            }
            drop(guard);
            (self.total.get() <= 0) as i32
        } else {
			let task = import_js_task.pop_front();
            let msg = js_task.pop_front();
            drop(guard);
            if let Some(task) = task {
				task.call();
            }
            if let Some(msg) = msg {
				msg.call();
            }
            0
        }
    }

    fn get_import_js_task_mut(&self) -> &mut VecDeque<ImportJsFuncMessage> {
        unsafe { &mut *self.import_js_task.get() }
    }

    fn get_js_task_mut(&self) -> &mut VecDeque<JsFuncMessage> {
        unsafe { &mut *self.js_tasks.get() }
    }
}
