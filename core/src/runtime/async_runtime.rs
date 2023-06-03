#[cfg(not(feature = "quickjs-libc"))]
use super::{Executor, Idle, Spawner};
use super::{Inner, Opaque};
#[cfg(feature = "quickjs-libc")]
use super::ThreadTaskSpawner;
use crate::Runtime;
#[cfg(not(feature = "quickjs-libc"))]
use crate::ParallelSend;
#[cfg(not(feature = "quickjs-libc"))]
use std::future::Future;

#[cfg(not(feature = "quickjs-libc"))]
/// The trait to spawn execution of pending jobs on async runtime
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
pub trait ExecutorSpawner: Sized {
    /// The type of join handle which returns `()`
    type JoinHandle;

    /// Spawn pending jobs using async runtime spawn function
    fn spawn_executor(self, task: Executor) -> Self::JoinHandle;
}

#[cfg(not(feature = "quickjs-libc"))]
macro_rules! async_rt_impl {
    ($($(#[$meta:meta])* $type:ident { $join_handle:ty, $spawn_local:path, $spawn:path })*) => {
        $(
            $(#[$meta])*
            pub struct $type;

            $(#[$meta])*
            impl ExecutorSpawner for $type {
                type JoinHandle = $join_handle;

                fn spawn_executor(
                    self,
                    task: Executor,
                ) -> Self::JoinHandle {
                    #[cfg(not(feature = "parallel"))]
                    use $spawn_local as spawn_parallel;
                    #[cfg(feature = "parallel")]
                    use $spawn as spawn_parallel;

                    spawn_parallel(task)
                }
            }
        )*
    };
}

#[cfg(not(feature = "quickjs-libc"))]
async_rt_impl! {
    /// The [`tokio`] async runtime for spawning executors.
    #[cfg(feature = "tokio")]
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "tokio")))]
    Tokio { tokio::task::JoinHandle<()>, tokio::task::spawn_local, tokio::task::spawn }

    /// The [`async_std`] runtime for spawning executors.
    #[cfg(feature = "async-std")]
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "async-std")))]
    AsyncStd { async_std::task::JoinHandle<()>, async_std::task::spawn_local, async_std::task::spawn }

    // The [`smol`] async runtime for spawning executors.
    #[cfg(not(feature = "quickjs-libc"))]
    #[cfg(any(feature = "smol", feature = "parallel"))]
    #[cfg_attr(feature = "doc-cfg", doc(cfg(all(feature = "smol", feature = "parallel"))))]
    Smol { smol::Task<()>, smol::spawn, smol::spawn }
}

#[cfg(not(feature = "quickjs-libc"))]
#[cfg(all(feature = "smol", feature = "parallel"))]
use smol::Executor as SmolExecutor;
#[cfg(not(feature = "quickjs-libc"))]
#[cfg(all(feature = "smol", not(feature = "parallel")))]
use smol::LocalExecutor as SmolExecutor;

#[cfg(not(feature = "quickjs-libc"))]
#[cfg(feature = "smol")]
impl<'a> ExecutorSpawner for &SmolExecutor<'a> {
    type JoinHandle = smol::Task<()>;

    fn spawn_executor(self, task: Executor) -> Self::JoinHandle {
        self.spawn(task)
    }
}

impl Inner {
    #[cfg(not(feature = "quickjs-libc"))]
    pub fn has_spawner(&self) -> bool {
        unsafe { self.get_opaque() }.spawner.is_some()
    }
}

impl Opaque {
    #[cfg(not(feature = "quickjs-libc"))]
    pub fn get_spawner(&self) -> &Spawner {
        self.spawner
            .as_ref()
            .expect("Async executor is not initialized for the Runtime. Possibly missing call `Runtime::run_executor()` or `Runtime::spawn_executor()`")
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn get_thread_spawner(&mut self) -> &mut ThreadTaskSpawner {
        self.thread_task_spawner
            .as_mut()
            .expect("Muti-thread components are not initialized for the Runtime.")
    }
}

impl Runtime {
    #[cfg(not(feature = "quickjs-libc"))]
    fn get_spawner(&self) -> &Spawner {
        let inner = self.inner.lock();
        let opaque = unsafe { &*(inner.get_opaque() as *const Opaque) };
        opaque.get_spawner()
    }

    #[cfg(not(feature = "quickjs-libc"))]
    /// Await until all pending jobs and spawned futures will be done
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    #[inline(always)]
    pub fn idle(&self) -> Idle {
        self.get_spawner().idle()
    }

    #[cfg(not(feature = "quickjs-libc"))]
    /// Run pending jobs and futures executor
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    #[inline(always)]
    pub fn run_executor(&self) -> Executor {
        let mut inner = self.inner.lock();
        let opaque = unsafe { &mut *(inner.get_opaque_mut() as *mut Opaque) };
        assert!(
            opaque.spawner.is_none(),
            "Async executor already initialized for the Runtime."
        );
        let (executor, spawner) = Executor::new();
        opaque.spawner = Some(spawner);
        executor
    }

    #[cfg(not(feature = "quickjs-libc"))]
    /// Spawn pending jobs and futures executor
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    #[inline(always)]
    pub fn spawn_executor<A: ExecutorSpawner>(&self, spawner: A) -> A::JoinHandle {
        spawner.spawn_executor(self.run_executor())
    }

    #[cfg(feature = "quickjs-libc")]
    pub fn init_exec_in_thread(&self) {
        use crate::runtime::ThreadRustTaskExecutor;

        let mut inner = self.inner.lock();
        let opaque = unsafe { &mut *(inner.get_opaque_mut() as *mut Opaque) };
        assert!(
            opaque.thread_task_spawner.is_none(),
            "Multi-thread components already initialized for the Runtime"
        );
        assert!(
            opaque.thread_js_task_executor.is_none(),
            "Multi-thread components already initialized for the Runtime"
        );
        let (
            thread_rust_task_exec, 
            thread_task_spawner, 
            thread_js_task_exec
        ) = ThreadRustTaskExecutor::new();
        opaque.thread_task_spawner = Some(thread_task_spawner);
        opaque.thread_js_task_executor = Some(thread_js_task_exec);
        opaque.exec_fn = Some(Box::new(move || {
            smol::block_on(thread_rust_task_exec);
        }));
    }

    #[cfg(not(feature = "quickjs-libc"))]
    pub(crate) fn spawn_pending_jobs(&self) {
        let runtime = self.clone();
        self.spawn(async move { runtime.execute_pending_jobs().await });
    }

    #[cfg(not(feature = "quickjs-libc"))]
    async fn execute_pending_jobs(&self) {
        loop {
            match self.execute_pending_job() {
                // No tasks in queue
                Ok(false) => break,
                // Task was executed successfully
                Ok(true) => (),
                // Task was failed with exception
                Err(error) => {
                    eprintln!("Error when pending job executing: {error}");
                }
            }
            futures_lite::future::yield_now().await;
        }
    }

    /// Spawn future using runtime
    #[cfg(not(feature = "quickjs-libc"))]
    #[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
    pub fn spawn<F, T>(&self, future: F)
    where
        F: Future<Output = T> + ParallelSend + 'static,
        T: ParallelSend + 'static,
    {
        self.get_spawner().spawn(future);
    }
}
