#[cfg(not(feature = "quickjs-libc"))]
use crate::{ParallelSend, Ref};
#[cfg(feature = "quickjs-libc")]
use crate::{SendSyncContext, SendSyncJsValue, WasmMessageCtx, JsMessageCtx, Result};
use async_task::Runnable;

#[cfg(not(feature = "quickjs-libc"))]
use flume::{r#async::RecvStream, unbounded, Receiver, Sender};

#[cfg(not(feature = "quickjs-libc"))]
use futures_lite::Stream;

#[cfg(feature = "quickjs-libc")]
use crate::qjs;

#[cfg(feature = "quickjs-libc")]
use futures_lite::FutureExt;

#[cfg(not(feature = "quickjs-libc"))]
use std::{pin::Pin, task::{Waker, Context, Poll}, sync::atomic::AtomicBool};

#[cfg(not(feature = "quickjs-libc"))]
use pin_project_lite::pin_project;
use std::{
    future::Future, sync::atomic::Ordering
};

#[cfg(feature = "quickjs-libc")]
use std::{
    any::Any, 
    sync::{atomic::AtomicI32, Arc},
    panic::AssertUnwindSafe,
    collections::VecDeque,
    cell::UnsafeCell
};

#[cfg(feature = "quickjs-libc-test")]
use libc::{sched_get_priority_max, sched_param, sched_setscheduler, SCHED_FIFO};

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
    promise_func: Box<dyn FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>)>
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
    wasm_tasks: crossbeam_channel::Receiver<WasmFuncMessage>,
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
pub struct AsyncCtx {
    rt: *mut qjs::JSRuntime,
    rust_exec: Option<ThreadRustTaskExecutor>,
    pub spawner: ThreadTaskSpawner,
    js_exec: ThreadJsTaskExecutor,
    wasm_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "quickjs-libc")]
impl AsyncCtx {
    pub(crate) fn new(rt: *mut qjs::JSRuntime) -> Self {
        let js_task_tx = Arc::new(UnsafeCell::new(VecDeque::with_capacity(100)));
        let js_task_rx = js_task_tx.clone();
        let (wasm_task_tx, wasm_task_rx) = crossbeam_channel::unbounded();
        let import_js_task_tx = Arc::new(UnsafeCell::new(Vec::with_capacity(1)));
        let import_js_task_rx = import_js_task_tx.clone();
        let total = Arc::new(AtomicI32::new(0));

        let mutex = Arc::new(std::sync::Mutex::new(false));
        
        let spawner = ThreadTaskSpawner {
            pipe: None,
            js_tasks: js_task_tx,
            wasm_tasks: wasm_task_tx,
            import_js_task: import_js_task_tx,
            total: total.clone(),

            mutex: mutex.clone()
        };

        let rust_exec = ThreadRustTaskExecutor {
            wasm_tasks: wasm_task_rx,
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
            rust_exec: Some(rust_exec),
            spawner,
            js_exec,
            wasm_thread: None,
        }
    }

    // spawn_thread will be called only once
    fn spawn_thread(&mut self, rt: *mut qjs::JSRuntime) {
        let pipe = unsafe { qjs::JS_CreateRustMessagePipe(rt) };
        if pipe.is_null() {
            panic!("failed creating rust message pipe");
        }
        let pipe = RustMsgPipe(pipe);
        let mut executor = self.rust_exec.take().unwrap();
        let handle = std::thread::spawn(move || {
            #[cfg(feature = "quickjs-libc-test")]
            unsafe {
                let param = sched_param {
                    sched_priority: sched_get_priority_max(SCHED_FIFO)
                };
                sched_setscheduler(0, SCHED_FIFO, &param);
            }
            executor.run();
        });
        self.spawner.pipe = Some(pipe.clone());
        self.js_exec.pipe = Some(pipe);
        self.wasm_thread = Some(handle);
    }

    pub fn spawn_wasm_task(
        &mut self, 
        ctx: crate::Context, 
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: impl FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>) + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
        if self.wasm_thread.is_none() {
            self.spawn_thread(self.rt);
        }
        let async_ctx = ctx.async_ctx_mut() as *mut AsyncCtx;
        let send_sync_ctx = SendSyncContext::new(ctx);
        self.spawner.spawn_wasm_task(
            send_sync_ctx,
            async_ctx,
            func, 
            Box::new(promise_func),
            resolve, 
            reject, 
            new_task
        );
    }

    pub fn spawn_js_task(
        &mut self, 
        ctx: crate::Context, 
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: impl FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>) + 'static,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
        let async_ctx = ctx.async_ctx_mut() as *mut AsyncCtx;
        let send_sync_ctx = SendSyncContext::new(ctx);
        self.spawner.spawn_js_task(
            send_sync_ctx, 
            async_ctx,
            func, 
            Box::new(promise_func),
            resolve, 
            reject, 
            new_task
        );
    }

    pub fn spawn_import_js_task<F>(&self, future: F, new_task: bool) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static
    {
        self.spawner.spawn_import_js_task(future, new_task)
    }

    pub(crate) fn run_js_single_task(&mut self) -> i32 {
        self.js_exec.run()
    }

    pub fn has_created_thread(&self) -> bool {
        self.wasm_thread.is_some()
    }

    pub fn is_main_thread(&self) -> bool {
        if let Some(handle) = &self.wasm_thread {
            handle.thread().id() != std::thread::current().id()
        } else {
            true
        }
    }

    pub(crate) fn join_handle(mut self) {
        drop(self.spawner);
        if let Some(handle) = self.wasm_thread.take() {
            handle.join().unwrap();
        }
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadTaskSpawner {
    pipe: Option<RustMsgPipe>,
    js_tasks: Arc<UnsafeCell<VecDeque<JsFuncMessage>>>,
    import_js_task: Arc<UnsafeCell<Vec<Runnable>>>,
    wasm_tasks: crossbeam_channel::Sender<WasmFuncMessage>,
    total: Arc<AtomicI32>,
    mutex: Arc<std::sync::Mutex<bool>>,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadTaskSpawner {
    pub fn spawn_wasm_task(
        &self, 
        ctx: SendSyncContext, 
        async_ctx: *mut AsyncCtx,
        func: Option<Box<dyn FnOnce() -> Result<Box<dyn Any + Send + 'static>> + Send + 'static>>,
        promise_func: Box<dyn FnOnce(WasmMessageCtx, Option<Result<Box<dyn Any + Send + 'static>>>)>,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
        let msg = WasmFuncMessage {
            ctx: WasmMessageCtx::new(ctx, async_ctx, resolve, reject),
            func,
            promise_func,
        };
        if new_task {
            self.total.fetch_add(1, Ordering::Relaxed);
        }
        self.wasm_tasks.send(msg).unwrap();
    }

    pub fn spawn_js_task(
        &self, 
        ctx: SendSyncContext, 
        async_ctx: *mut AsyncCtx,
        func: Option<Box<dyn FnOnce(&JsMessageCtx) -> Result<Box<dyn Any + 'static>> + 'static>>,
        promise_func: Box<dyn FnOnce(JsMessageCtx, Option<Result<Box<dyn Any + 'static>>>)>,
        resolve: SendSyncJsValue,
        reject: SendSyncJsValue,
        new_task: bool
    ) {
        let msg = JsFuncMessage {
            ctx: JsMessageCtx::new(ctx, async_ctx, resolve, reject, self.total.clone()),
            func,
            promise_func
        };
        if new_task {
            self.total.fetch_add(1, Ordering::Relaxed);
        }
        let js_task = self.get_js_task_mut();
        let guard = self.mutex.lock().unwrap();
        if js_task.is_empty() {
            self.write_js_pipe();
        }
        js_task.push_back(msg);
        drop(guard);
    }

    pub fn spawn_import_js_task<F>(
        &self,
        future: F,
        new_task: bool
    ) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        if new_task {
            self.total.fetch_add(1, Ordering::Relaxed);
        }
        let total = self.total.clone();
        let (runnable, task) = async_task::spawn(
            async move {
                let output = AssertUnwindSafe(future).catch_unwind().await;
                total.fetch_sub(1, Ordering::Relaxed);
                match output {
                    Ok(output) => output,
                    Err(err) => std::panic::resume_unwind(err),
                }
            },
            |_| unreachable!(),
        );

        let guard = self.mutex.lock().unwrap();
        if self.get_js_task_mut().is_empty() {
            self.write_js_pipe();
        }
        self.get_import_js_task_mut().push(runnable);
        drop(guard);
        task
    }

    pub fn write_js_pipe(&self) {
        let pipe = self.pipe.as_ref().unwrap();
        unsafe { qjs::JS_WriteRustMessagePipe(pipe.0); }
    }

    fn get_js_task_mut(&self) -> &mut VecDeque<JsFuncMessage> {
        unsafe { &mut *self.js_tasks.get() }
    }

    fn get_import_js_task_mut(&self) -> &mut Vec<Runnable> {
        unsafe { &mut *self.import_js_task.get() }
    }
}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadJsTaskExecutor {
    pipe: Option<RustMsgPipe>,
    js_tasks: Arc<UnsafeCell<VecDeque<JsFuncMessage>>>,
    import_js_task: Arc<UnsafeCell<Vec<Runnable>>>,
    total: Arc<AtomicI32>,
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
            let res = self.total.load(Ordering::Acquire);
            (res <= 0) as i32
        } else {
            let task = import_js_task.pop();
            let msg = js_task.pop_front();
            drop(guard);
            if let Some(task) = task {
                task.run();
            }
            if let Some(msg) = msg {
                msg.call();
            }
            0
        }
    }

    fn get_import_js_task_mut(&self) -> &mut Vec<Runnable> {
        unsafe { &mut *self.import_js_task.get() }
    }

    fn get_js_task_mut(&self) -> &mut VecDeque<JsFuncMessage> {
        unsafe { &mut *self.js_tasks.get() }
    }
}
