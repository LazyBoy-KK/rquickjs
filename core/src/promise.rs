use crate::{
    Ctx, Error, FromJs, Func, Function, Mut, Object, ParallelSend,
    Ref, Result, This, Value,
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context as TaskContext, Poll, Waker},
};

#[cfg(not(feature = "quickjs-libc"))]
use crate::{Context, IntoJs, Persistent};

/// Future-aware promise
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
pub struct Promise<T> {
    state: Ref<Mut<State<T>>>,
}

struct State<T> {
    result: Option<Result<T>>,
    waker: Option<Waker>,
}

impl<T> State<T> {
    fn resolve(&mut self, result: Result<T>) {
        self.result = Some(result);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            result: None,
            waker: None,
        }
    }
}

impl<'js, T> FromJs<'js> for Promise<T>
where
    T: FromJs<'js> + ParallelSend + 'static,
{
    fn from_js(_ctx: Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let obj = Object::from_value(value)?;
        let then: Function = obj.get("then")?;
        let state = Ref::new(Mut::new(State::default()));
        let on_ok = Func::new("onSuccess", {
            let state = state.clone();
            move |ctx: Ctx<'js>, value: Value<'js>| {
                let mut state = state.lock();
                state.resolve(T::from_js(ctx, value));
            }
        });
        let on_err = Func::new("onError", {
            let state = state.clone();
            move |error: Error| {
                let mut state = state.lock();
                state.resolve(Err(error));
            }
        });
        then.call((This(obj), on_ok, on_err))?;
        Ok(Self { state })
    }
}

impl<T> Future for Promise<T> {
    type Output = Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext) -> Poll<Self::Output> {
        let mut state = self.state.lock();
        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(not(feature = "quickjs-libc"))]
/// Wrapper for futures to convert to JS promises
#[cfg_attr(feature = "doc-cfg", doc(cfg(feature = "futures")))]
#[repr(transparent)]
pub struct Promised<T>(pub T);

#[cfg(not(feature = "quickjs-libc"))]
impl<T> From<T> for Promised<T> {
    fn from(future: T) -> Self {
        Self(future)
    }
}

#[cfg(not(feature = "quickjs-libc"))]
impl<'js, T> IntoJs<'js> for Promised<T>
where
    T: Future + ParallelSend + 'static,
    for<'js_> T::Output: IntoJs<'js_> + 'static,
{
    fn into_js(self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let future = self.0;
        let (promise, resolve, reject) = ctx.promise()?;

        let resolve = Persistent::save(ctx, resolve);
        let reject = Persistent::save(ctx, reject);
        let context = Context::from_ctx(ctx)?;
        ctx.spawn(async move {
            let value = future.await;
            context.with(|ctx| {
                let (func, value) = match value.into_js(ctx) {
                    Ok(value) => (resolve.clone().restore(ctx).unwrap(), value),
                    Err(error) => (
                        reject.clone().restore(ctx).unwrap(),
                        error.into_js(ctx).unwrap(),
                    ),
                };
                func.call::<_, Value>((value,)).unwrap();
            });
        });

        Ok(promise.into_value())
    }
}

#[cfg(all(test, any(feature = "async-std", feature = "tokio")))]
mod test {
    use crate::*;
    use futures_rs::prelude::*;

    macro_rules! test_cases {
	      ($($name:ident ($ctx:ident) { $($content:tt)* })*) => {
            #[cfg(feature = "tokio")]
            mod tokio_tests {
                use super::*;

                async fn delayed<T>(msec: u32, value: T) -> T {
                    let dur = std::time::Duration::from_millis(msec as _);
                    tokio_rs::time::sleep(dur).await;
                    value
                }

                $(
                    #[tokio::test]
                    async fn $name() {
                        #[cfg(not(feature = "parallel"))]
                        {
                            tokio::task::LocalSet::new().run_until(async {
                                let rt = Runtime::new().unwrap();
                                let $ctx = Context::full(&rt).unwrap();

                                rt.spawn_executor(Tokio);

                                $($content)*

                                rt.idle().await;
                            }).await;
                        }
                        #[cfg(feature = "parallel")]
                        {
                            let rt = Runtime::new().unwrap();
                            let $ctx = Context::full(&rt).unwrap();

                            rt.spawn_executor(Tokio);

                            $($content)*

                            rt.idle().await;
                        }
                    }
                )*
            }

            #[cfg(feature = "async-std")]
            mod async_std_tests {
                use super::*;

                async fn delayed<T>(msec: u32, value: T) -> T {
                    let dur = std::time::Duration::from_millis(msec as _);
                    async_std_rs::task::sleep(dur).await;
                    value
                }

                $(
                    #[async_std::test]
                    async fn $name() {
                        let rt = Runtime::new().unwrap();
                        let $ctx = Context::full(&rt).unwrap();

                        rt.spawn_executor(AsyncStd);

                        $($content)*

                        rt.idle().await;
                    }
                )*
            }
	      };
    }

    test_cases! {
        delayed_fn_self (_ctx) {
            let res1 = delayed(25, 1).into_stream();
            let res2 = delayed(50, 2).into_stream();
            let res3 = delayed(75, 3).into_stream();

            let res = res1.chain(res2).chain(res3).collect::<Vec<_>>().await;
            assert_eq!(res, &[1, 2, 3]);
        }

        delayed_fn_single (ctx) {
            let res: Promise<i32> = ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set(
                        "delayed",
                        //Func::from(|msec, data: i32| Promised(delayed(msec, data))),
                        Func::from(Async(delayed::<i32>)),
                    )
                    .unwrap();
                ctx.eval("delayed(50, 5)").unwrap()
            });

            let res = res.await.unwrap();
            assert_eq!(res, 5);
        }

        delayed_fn_swarm (ctx) {
            let res: Promise<i32> = ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set(
                        "delayed",
                        //Func::from(|msec, data: i32| Promised(delayed(msec, data))),
                        Func::from(Async(delayed::<i32>)),
                    )
                    .unwrap();
                let test: Function = ctx.eval(r#"
async (iterations, min_parallel, max_parallel, min_timeout, max_timeout) => {
    for (let i = 0; i < iterations; i++) {
        let parallel = Math.round(Math.random() * (max_parallel - min_parallel) + min_parallel);
        let promises = Array.from({length: parallel}, () => delayed(Math.random() * (max_timeout - min_timeout) + min_timeout, 0));
        await Promise.all(promises);
    }
    return 42;
}
"#).unwrap();
                test.call((15, 100, 1000, 0, 15)).unwrap()
            });

            let res = res.await.unwrap();
            assert_eq!(res, 42);
        }

        delayed_fn (ctx) {
            let res: Promise<i32> = ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set(
                        "delayed",
                        Func::from(|msec, data: i32| Promised(delayed(msec, data))),
                    )
                    .unwrap();
                ctx.eval("delayed(50, 2)").unwrap()
            });

            let res2 = async { res.await.unwrap() }.into_stream();
            let res1 = delayed(25, 1).into_stream();
            let res3 = delayed(75, 3).into_stream();

            let res = res1.chain(res2).chain(res3).collect::<Vec<_>>().await;
            assert_eq!(res, &[1, 2, 3]);
        }

        async_fn_no_throw (ctx) {
            async fn mul2(a: i32, b: i32) -> i32 {
                a * b
            }

            let res: Promise<i32> = ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set("mul2", Func::from(|a, b| Promised(mul2(a, b))))
                    .unwrap();
                ctx.eval("mul2(2, 3)").unwrap()
            });

            let res = res.await.unwrap();
            assert_eq!(res, 6);
        }

        unhandled_promise_js (ctx) {
            async fn doit() {}

            ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set("doit", Func::from(|| Promised(doit())))
                    .unwrap();
                let _ = ctx.eval::<Value, _>("doit()").unwrap();
            });
        }

        unhandled_promise_future (ctx) {
            async fn doit() {}

            let _res: Promise<()> = ctx.with(|ctx| {
                let global = ctx.globals();
                global
                    .set("doit", Func::from(|| Promised(doit())))
                    .unwrap();
                ctx.eval("doit()").unwrap()
            });
        }
    }
}
