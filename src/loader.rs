use crate::{qjs, Ctx, Error, Module, Result};
use std::{
    ffi::CStr,
    fs::read,
    path::{Path, PathBuf},
    ptr,
};

/// Module loader trait
///
/// # Features
/// This trait is only availble if the `loader` feature is enabled.
pub trait Loader {
    /// Normalize module name
    ///
    /// The default normalization looks like:
    ///
    /// ```no_run
    /// # use std::path::{Path, PathBuf};
    /// # use rquickjs::{Ctx, Result, Error};
    /// # fn default_normalize<'js>(_ctx: Ctx<'js>, base: &Path, name: &Path) -> Result<PathBuf> {
    /// Ok(if !name.starts_with(".") {
    ///     name.into()
    /// } else {
    ///     base.parent()
    ///         .ok_or(Error::Unknown)?
    ///         .join(name)
    ///         .canonicalize()?
    /// })
    /// # }
    /// ```
    fn normalize<'js>(&mut self, ctx: Ctx<'js>, base: &Path, name: &Path) -> Result<PathBuf>;

    /// Load module by name
    ///
    /// The example loading may looks like:
    ///
    /// ```no_run
    /// # use std::{fs::read, path::Path};
    /// # use rquickjs::{Ctx, Module, Result};
    /// # fn default_load<'js>(ctx: Ctx<'js>, path: &Path) -> Result<Module<'js>> {
    /// let name = path.to_string_lossy();
    /// let source: Vec<_> = read(path)?;
    /// ctx.compile(name.as_ref(), source)
    /// # }
    /// ```
    fn load<'js>(&mut self, ctx: Ctx<'js>, name: &Path) -> Result<Module<'js>>;
}

type DynLoader = Box<dyn Loader>;

#[repr(transparent)]
pub(crate) struct LoaderHolder(*mut DynLoader);

impl Drop for LoaderHolder {
    fn drop(&mut self) {
        let _loader = unsafe { Box::from_raw(self.0) };
    }
}

impl LoaderHolder {
    pub fn new<L>(loader: L) -> Self
    where
        L: Loader + 'static,
    {
        Self(Box::into_raw(Box::new(Box::new(loader))))
    }

    pub(crate) fn set_to_runtime(&self, rt: *mut qjs::JSRuntime) {
        unsafe {
            qjs::JS_SetModuleLoaderFunc(
                rt,
                Some(Self::normalize_raw),
                Some(Self::load_raw),
                self.0 as _,
            );
        }
    }

    #[inline]
    fn normalize<'js>(
        loader: &mut DynLoader,
        ctx: Ctx<'js>,
        base: &CStr,
        name: &CStr,
    ) -> Result<*mut qjs::c_char> {
        let base = Path::new(base.to_str()?);
        let name = Path::new(name.to_str()?);

        let name = loader.normalize(ctx, &base, &name)?;
        let name = name.to_string_lossy();

        // We should transfer ownership of this string to QuickJS
        Ok(unsafe { qjs::js_strndup(ctx.ctx, name.as_ptr() as _, name.as_bytes().len() as _) })
    }

    unsafe extern "C" fn normalize_raw(
        ctx: *mut qjs::JSContext,
        base: *const qjs::c_char,
        name: *const qjs::c_char,
        opaque: *mut qjs::c_void,
    ) -> *mut qjs::c_char {
        let ctx = Ctx::from_ptr(ctx);
        let base = CStr::from_ptr(base);
        let name = CStr::from_ptr(name);
        let loader = &mut *(opaque as *mut DynLoader);

        Self::normalize(loader, ctx, &base, &name).unwrap_or_else(|_| ptr::null_mut())
    }

    #[inline]
    fn load<'js>(
        loader: &mut DynLoader,
        ctx: Ctx<'js>,
        name: &CStr,
    ) -> Result<*mut qjs::JSModuleDef> {
        let name = Path::new(name.to_str()?);

        Ok(loader.load(ctx, name)?.as_module_def())
    }

    unsafe extern "C" fn load_raw(
        ctx: *mut qjs::JSContext,
        name: *const qjs::c_char,
        opaque: *mut qjs::c_void,
    ) -> *mut qjs::JSModuleDef {
        let ctx = Ctx::from_ptr(ctx);
        let name = CStr::from_ptr(name);
        let loader = &mut *(opaque as *mut DynLoader);

        Self::load(loader, ctx, &name).unwrap_or_else(|_| ptr::null_mut())
    }
}

/// The default module loader
///
/// This loader can be used as the nested backing loader in user-defined loaders.
pub struct DefaultLoader;

impl Loader for DefaultLoader {
    fn normalize<'js>(&mut self, _ctx: Ctx<'js>, base: &Path, name: &Path) -> Result<PathBuf> {
        Ok(if !name.starts_with(".") {
            name.into()
        } else {
            base.parent()
                .ok_or(Error::Unknown)?
                .join(name)
                .canonicalize()?
        })
    }

    fn load<'js>(&mut self, ctx: Ctx<'js>, path: &Path) -> Result<Module<'js>> {
        let name = path.to_string_lossy();
        let source: Vec<_> = read(path)?;
        ctx.compile(name.as_ref(), source)
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn user_loader() {
        struct TestLoader;
        impl Loader for TestLoader {
            fn normalize<'js>(
                &mut self,
                _ctx: Ctx<'js>,
                base: &Path,
                name: &Path,
            ) -> Result<PathBuf> {
                assert_eq!(base, Path::new("test_loader"));
                assert_eq!(name, Path::new("test"));
                Ok(name.into())
            }

            fn load<'js>(&mut self, ctx: Ctx<'js>, path: &Path) -> Result<Module<'js>> {
                assert_eq!(path, Path::new("test"));
                ctx.compile(
                    "test",
                    r#"
                      export const n = 123;
                      export const s = "abc";
                    "#,
                )
            }
        }

        let rt = Runtime::new().unwrap();
        rt.set_loader(TestLoader);
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            eprintln!("test");
            let _module = ctx
                .compile(
                    "test_loader",
                    r#"
                      import { n, s } from "test";
                      export default [n, s];
                    "#,
                )
                .unwrap();
        })
    }
}
