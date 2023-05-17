use crate::{ParallelSend, Ref};
use async_task::Runnable;
use flume::{r#async::RecvStream, unbounded, Receiver, Sender};
use futures_lite::Stream;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::{AtomicBool, Ordering}},
    task::{Context, Poll, Waker},
};

#[cfg(feature = "quickjs-libc-threads")]
use std::sync::{Arc, atomic::AtomicI32};

#[cfg(feature = "parallel")]
use async_task::spawn as spawn_task;
#[cfg(not(feature = "parallel"))]
use async_task::spawn_local as spawn_task;

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

pub struct Spawner {
    tasks: Sender<Runnable>,
    idles: Sender<Waker>,
    idle: Ref<AtomicBool>,
}

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

struct InnerIdle {
    idle: Ref<AtomicBool>,
    signal: Sender<Waker>,
}

/// The idle awaiting future
#[derive(Default)]
pub struct Idle(Option<InnerIdle>);

impl Idle {
    fn new(idle: &Ref<AtomicBool>, sender: &Sender<Waker>) -> Self {
        Self(Some(InnerIdle {
            idle: idle.clone(),
            signal: sender.clone(),
        }))
    }
}

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

#[cfg(feature = "quickjs-libc-threads")]
pin_project! {
    pub struct ThreadRustTaskExecutor {
        #[pin]
        rust_tasks: RecvStream<'static, Runnable>,
    }
}

#[cfg(feature = "quickjs-libc-threads")]
impl ThreadRustTaskExecutor {
    pub(crate) fn new() -> (Self, ThreadTaskSpawner, ThreadJsTaskExecutor) {
        let (js_task_tx, js_task_rx) = unbounded();
        let (rust_task_tx, rust_task_rx) = unbounded();
        let total = Arc::new(AtomicI32::new(0));
        (
            Self {
                rust_tasks: rust_task_rx.into_stream(),
            },
            ThreadTaskSpawner {
                js_tasks: js_task_tx,
                rust_tasks: rust_task_tx,
                total: total.clone(),
            },
            ThreadJsTaskExecutor {
                js_tasks: js_task_rx.into_stream(),
                total,
            }
        )
    }
}

#[cfg(feature = "quickjs-libc-threads")]
impl Future for ThreadRustTaskExecutor {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if let Poll::Ready(task) = self.as_mut().project().rust_tasks.poll_next(cx) {
            if let Some(task) = task {
                task.run();
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(feature = "quickjs-libc-threads")]
#[derive(Clone)]
pub struct ThreadTaskSpawner {
    js_tasks: Sender<Runnable>,
    rust_tasks: Sender<Runnable>,
    pub total: Arc<AtomicI32>,
}

#[cfg(feature = "quickjs-libc-threads")]
impl ThreadTaskSpawner {
    pub fn spawn_rust_task<F>(&mut self, future: F) -> async_task::Task<<F as Future>::Output>
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
        move |runnable| {
            rust_task
                .send(runnable)
                .expect("Rust task executor unexpectly destroyed");
        }
    }
}

#[cfg(feature = "quickjs-libc-threads")]
pin_project! {
    pub struct ThreadJsTaskExecutor {
        #[pin]
        js_tasks: RecvStream<'static, Runnable>,
        total: Arc<AtomicI32>,
    }
}

#[cfg(feature = "quickjs-libc-threads")]
impl Future for ThreadJsTaskExecutor {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if let Poll::Ready(task) = self.as_mut().project().js_tasks.poll_next(cx) {
            if let Some(task) = task {
                task.run();
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }
        Poll::Ready(())
    }
}