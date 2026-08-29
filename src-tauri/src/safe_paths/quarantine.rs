use std::{io, path::Path};

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::{
        ffi::{c_void, CString},
        os::{raw::c_char, unix::ffi::OsStrExt},
        ptr,
    };

    const QUARANTINE_NAME: &[u8] = b"com.apple.quarantine\0";

    unsafe extern "C" {
        fn listxattr(path: *const c_char, namebuf: *mut c_char, size: usize, options: i32)
            -> isize;
        fn getxattr(
            path: *const c_char,
            name: *const c_char,
            value: *mut c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn setxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
    }

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path contains an interior NUL")
        })
    }

    pub(super) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
        let path = c_path(path)?;
        let list_size = unsafe { listxattr(path.as_ptr(), ptr::null_mut(), 0, 0) };
        if list_size < 0 {
            return Err(io::Error::last_os_error());
        }
        if list_size == 0 {
            return Ok(None);
        }

        let mut names = vec![0_u8; list_size as usize];
        let listed = unsafe {
            listxattr(
                path.as_ptr(),
                names.as_mut_ptr().cast::<c_char>(),
                names.len(),
                0,
            )
        };
        if listed < 0 {
            return Err(io::Error::last_os_error());
        }
        names.truncate(listed as usize);
        if !names
            .split(|byte| *byte == 0)
            .any(|name| name == b"com.apple.quarantine")
        {
            return Ok(None);
        }

        let name = QUARANTINE_NAME.as_ptr().cast::<c_char>();
        let value_size = unsafe { getxattr(path.as_ptr(), name, ptr::null_mut(), 0, 0, 0) };
        if value_size < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = vec![0_u8; value_size as usize];
        let read = unsafe {
            getxattr(
                path.as_ptr(),
                name,
                value.as_mut_ptr().cast::<c_void>(),
                value.len(),
                0,
                0,
            )
        };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        value.truncate(read as usize);
        Ok(Some(value))
    }

    pub(super) fn write(path: &Path, value: &[u8]) -> io::Result<()> {
        let path = c_path(path)?;
        let result = unsafe {
            setxattr(
                path.as_ptr(),
                QUARANTINE_NAME.as_ptr().cast::<c_char>(),
                value.as_ptr().cast::<c_void>(),
                value.len(),
                0,
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    imp::read(path)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read(_path: &Path) -> io::Result<Option<Vec<u8>>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
pub(crate) fn write(path: &Path, value: &[u8]) -> io::Result<()> {
    imp::write(path, value)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn write(_path: &Path, _value: &[u8]) -> io::Result<()> {
    Ok(())
}

pub(crate) fn copy(source: &Path, destination: &Path) -> io::Result<()> {
    let value = match read(source) {
        Ok(value) => value,
        Err(error) if xattrs_unsupported(&error) => None,
        Err(error) => return Err(error),
    };
    if let Some(value) = value {
        if let Err(error) = write(destination, &value) {
            if !xattrs_unsupported(&error) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xattrs_unsupported(error: &io::Error) -> bool {
    const ENOTSUP: i32 = 45;
    const EOPNOTSUPP: i32 = 102;
    matches!(error.raw_os_error(), Some(ENOTSUP | EOPNOTSUPP))
}

#[cfg(not(target_os = "macos"))]
fn xattrs_unsupported(_error: &io::Error) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn only_unsupported_xattr_errors_are_best_effort() {
        assert!(xattrs_unsupported(&io::Error::from_raw_os_error(45)));
        assert!(xattrs_unsupported(&io::Error::from_raw_os_error(102)));
        assert!(!xattrs_unsupported(&io::Error::from_raw_os_error(13)));
    }
}
