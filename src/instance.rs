use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

/// Process-wide lock for the GUI host. The lock file may remain after exit;
/// the kernel releases the advisory lock when its owning process terminates.
pub struct InstanceGuard {
    #[allow(dead_code)]
    file: File,
}

impl InstanceGuard {
    pub fn acquire() -> io::Result<Option<Self>> {
        let path = lock_path()?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;

        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EWOULDBLOCK)
                    || error.raw_os_error() == Some(libc::EAGAIN)
                {
                    return Ok(None);
                }
                return Err(error);
            }
        }

        Ok(Some(Self { file }))
    }
}

fn lock_path() -> io::Result<PathBuf> {
    let dir = crate::paths::data_dir().map_err(io::Error::other)?;
    Ok(dir.join("myproxy.instance.lock"))
}
