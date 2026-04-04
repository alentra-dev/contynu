#[cfg(unix)]
use std::ffi::{CString, OsStr, OsString};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use libc::{self, c_char, pid_t};

use crate::error::{ContynuError, Result};

#[cfg(unix)]
pub struct PtyChild {
    master: File,
    pid: pid_t,
}

#[cfg(unix)]
impl PtyChild {
    pub fn spawn(
        cwd: &Path,
        executable: &OsStr,
        args: &[OsString],
        env: &[(String, String)],
    ) -> Result<Self> {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        let rc = unsafe {
            libc::openpty(
                &mut master as *mut RawFd as *mut _,
                &mut slave as *mut RawFd as *mut _,
                std::ptr::null_mut(),
                std::ptr::null_mut() as *mut _,
                std::ptr::null_mut() as *mut _,
            )
        };
        if rc != 0 {
            return Err(ContynuError::CommandStart(format!(
                "openpty failed: {}",
                io::Error::last_os_error()
            )));
        }

        let argv = build_argv(executable, args)?;
        let cwd_c = CString::new(cwd.as_os_str().as_bytes()).map_err(|error| {
            ContynuError::Validation(format!("invalid cwd for PTY launch: {error}"))
        })?;

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(master);
                libc::close(slave);
            }
            return Err(ContynuError::CommandStart(format!(
                "fork failed: {}",
                io::Error::last_os_error()
            )));
        }

        if pid == 0 {
            unsafe {
                libc::setsid();
                libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                libc::close(master);
                libc::close(slave);
                libc::chdir(cwd_c.as_ptr());
                for (key, value) in env {
                    if let (Ok(key), Ok(value)) =
                        (CString::new(key.as_str()), CString::new(value.as_str()))
                    {
                        libc::setenv(key.as_ptr(), value.as_ptr(), 1);
                    }
                }
                let mut argv_ptrs = argv
                    .iter()
                    .map(|arg| arg.as_ptr())
                    .collect::<Vec<*const c_char>>();
                argv_ptrs.push(std::ptr::null());
                libc::execvp(argv[0].as_ptr(), argv_ptrs.as_ptr());
                libc::_exit(127);
            }
        }

        unsafe {
            libc::close(slave);
        }
        Ok(Self {
            master: unsafe { File::from_raw_fd(master) },
            pid,
        })
    }

    pub fn try_clone_reader(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    pub fn try_clone_writer(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    pub fn wait(&self) -> Result<PtyExitStatus> {
        let mut status = 0_i32;
        let rc = unsafe { libc::waitpid(self.pid, &mut status, 0) };
        if rc < 0 {
            return Err(ContynuError::CommandStart(format!(
                "waitpid failed: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(PtyExitStatus::from_wait_status(status))
    }

    pub fn interrupt(&self) {
        unsafe {
            libc::kill(-self.pid, libc::SIGTERM);
        }
    }

    pub fn pid(&self) -> pid_t {
        self.pid
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub struct PtyExitStatus {
    code: Option<i32>,
    success: bool,
}

#[cfg(unix)]
impl PtyExitStatus {
    fn from_wait_status(status: i32) -> Self {
        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            Self {
                code: Some(code),
                success: code == 0,
            }
        } else if libc::WIFSIGNALED(status) {
            Self {
                code: Some(128 + libc::WTERMSIG(status)),
                success: false,
            }
        } else {
            Self {
                code: None,
                success: false,
            }
        }
    }

    pub fn code(self) -> Option<i32> {
        self.code
    }

    pub fn success(self) -> bool {
        self.success
    }
}

#[cfg(unix)]
fn build_argv(executable: &OsStr, args: &[OsString]) -> Result<Vec<CString>> {
    let mut values = Vec::with_capacity(args.len() + 1);
    values.push(to_cstring(executable)?);
    for arg in args {
        values.push(to_cstring(arg)?);
    }
    Ok(values)
}

#[cfg(unix)]
fn to_cstring(value: &OsStr) -> Result<CString> {
    CString::new(value.as_bytes()).map_err(|error| {
        ContynuError::Validation(format!("invalid PTY launch argument contains NUL: {error}"))
    })
}

#[cfg(not(unix))]
pub struct PtyChild;

#[cfg(not(unix))]
impl PtyChild {
    pub fn spawn(
        _cwd: &std::path::Path,
        _executable: &std::ffi::OsStr,
        _args: &[std::ffi::OsString],
        _env: &[(String, String)],
    ) -> Result<Self> {
        Err(ContynuError::Unsupported(
            "PTY transport is only implemented on Unix".into(),
        ))
    }
}
