#[cfg(not(feature = "quickjs-libc"))]
use crate::{ParallelSend, Ref};

use async_task::Runnable;

#[cfg(not(feature = "quickjs-libc"))]
use flume::r#async::RecvStream;

#[cfg(not(feature = "quickjs-libc"))]
use futures_lite::Stream;

use flume::{unbounded, Sender, Receiver};

#[cfg(not(feature = "quickjs-libc"))]
use std::{task::Waker, pin::Pin};

#[cfg(not(feature = "quickjs-libc"))]
use pin_project_lite::pin_project;
use std::{
    future::Future,
    sync::{atomic::Ordering, atomic::AtomicBool},
    task::{Context, Poll},
};

#[cfg(feature = "quickjs-libc")]
use std::{
    sync::{Arc, atomic::AtomicI32},
    cell::UnsafeCell,
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
#[derive(Clone)]
pub struct ThreadRustTaskExecutor {
    rust_tasks: Arc<Receiver<Runnable>>,
    closed: Arc<AtomicBool>,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadRustTaskExecutor {
    pub fn spawn_thread(&self) -> std::thread::JoinHandle<()> {
        let exec = self.clone();
        std::thread::spawn(move || {
            exec.run(true);
        })
    }

    pub fn run(&self, is_spawning: bool) -> bool {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                break false;
            }
            match self.rust_tasks.try_recv() {
                Ok(task) => { task.run(); }
                Err(flume::TryRecvError::Empty) => {
                    if !is_spawning {
                        break true;
                    } else {
                        std::thread::park();
                    }
                }
                Err(flume::TryRecvError::Disconnected) => break false,
            }
        }
    }
}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct AsyncCtx {
    executor: ThreadRustTaskExecutor,
    spawner: ThreadTaskSpawner,
}

#[cfg(feature = "quickjs-libc")]
impl AsyncCtx {
    pub(crate) fn new() -> (Self, ThreadJsTaskExecutor) {
        let (js_task_tx, js_task_rx) = unbounded();
        let (rust_task_tx, rust_task_rx) = unbounded();
        let total = Arc::new(AtomicI32::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let rust_task_rx = Arc::new(rust_task_rx.clone());

        let spawner = ThreadTaskSpawner { 
            js_tasks: js_task_tx, 
            rust_tasks: rust_task_tx, 
            total: total.clone(),
            thread: Arc::new(UnsafeCell::new(None)),
        };

        let rust_exec = ThreadRustTaskExecutor {
            rust_tasks: rust_task_rx,
            closed,
        };

        (
            Self {
                executor: rust_exec.clone(), 
                spawner,
            },
            ThreadJsTaskExecutor {
                js_tasks: js_task_rx,
                total,
            }
        )
    }

    pub(crate) fn spawn_thread(&mut self) -> std::thread::JoinHandle<()> {
        let handle = self.executor.spawn_thread();
        *self.spawner.get_mut_thread() = Some(handle.thread().clone());
        handle
    }

    /// Poll a future and will not block on rust thread when the future is
    /// pending.
    /// Returns Some(Future::Output) when the future returns Poll::Ready(Output)
    /// Returns None when message queues have been dropped
    pub fn block_on<F>(&self, future: F) -> Option<F::Output>
    where
        F: Future
    {
        futures_lite::pin!(future);
        let thread = self.spawner.get_mut_thread().as_ref().unwrap().clone();
        let waker = waker_fn::waker_fn(move || {
            thread.unpark();
        });
        let mut cx = Context::from_waker(&waker);

        loop {
            if !self.executor.run(false) {
                break None;
            }
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(output) => return Some(output),
                Poll::Pending => std::thread::park(),
            }
        }
    }

    pub fn spawn_rust_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawner.spawn_rust_task(future)
    }

    pub fn spawn_js_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        self.spawner.spawn_js_task(future)
    }

    pub fn spawn_js_cross_thread_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawner.spawn_js_cross_thread_task(future)
    }

    pub(crate) fn close_channel(&mut self) {
        self.executor.closed.store(true, Ordering::SeqCst);
        self.spawner.unpark_thread();
    }
}

#[cfg(feature = "quickjs-libc")]
#[derive(Clone)]
pub struct ThreadTaskSpawner {
    js_tasks: Sender<Runnable>,
    rust_tasks: Sender<Runnable>,
    total: Arc<AtomicI32>,
    thread: Arc<UnsafeCell<Option<std::thread::Thread>>>,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadTaskSpawner {
    pub fn spawn_rust_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.total.fetch_add(1, Ordering::SeqCst);
        let total = self.total.clone();
        let (runnable, task) = async_task::spawn(
            async move {
                let output = future.await;
                total.fetch_sub(1, Ordering::SeqCst);
                output
            }, self.rust_tasks_schedule()
        );
        runnable.schedule();
        task
    }

    pub fn spawn_js_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        self.total.fetch_add(1, Ordering::SeqCst);
        let total = self.total.clone();
        let (runnable, task) = async_task::spawn_local(
            async move {
                let output = future.await;
                total.fetch_sub(1, Ordering::SeqCst);
                output
            }, self.js_tasks_schedule()
        );
        runnable.schedule();
        task
    }

    pub fn spawn_js_cross_thread_task<F>(&self, future: F) -> async_task::Task<<F as Future>::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.total.fetch_add(1, Ordering::SeqCst);
        let total = self.total.clone();
        let (runnable, task) = async_task::spawn(
            async move {
                let output = future.await;
                total.fetch_sub(1, Ordering::SeqCst);
                output
            }, self.js_tasks_schedule()
        );
        runnable.schedule();
        task
    }

    fn js_tasks_schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let js_task = self.js_tasks.clone();
        move |runnable| {
            js_task
                .send(runnable)
                .expect("JS task executor unexpectly destroyed");
        }
    }

    fn rust_tasks_schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let rust_task = self.rust_tasks.clone();
        let spawner = self.clone();
        move |runnable| {
            rust_task
                .send(runnable)
                .expect("Rust task executor unexpectly destroyed");
            spawner.unpark_thread();
        }
    }

    pub fn unpark_thread(&self) {
        if let Some(thread) = self.get_mut_thread() {
            thread.unpark();
        }
    }

    pub fn get_mut_thread(&self) -> &mut Option<std::thread::Thread> {
        unsafe { &mut *self.thread.get() }
    }
}

/// Thread handle in Spawner will be changed only once when creating 
/// rust thread and will not be dropped until freeing quickjs runtime
#[cfg(feature = "quickjs-libc")]
unsafe impl Send for ThreadTaskSpawner {}
#[cfg(feature = "quickjs-libc")]
unsafe impl Sync for ThreadTaskSpawner {}

#[cfg(feature = "quickjs-libc")]
pub struct ThreadJsTaskExecutor {
    js_tasks: Receiver<Runnable>,
    total: Arc<AtomicI32>,
}

#[cfg(feature = "quickjs-libc")]
impl ThreadJsTaskExecutor {
    pub fn run(&self) -> bool {
        if let Ok(task) = self.js_tasks.try_recv() {
            task.run();
        }
        self.total.load(Ordering::SeqCst) <= 0
    }
}