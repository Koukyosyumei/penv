#[cfg(target_os = "linux")]
mod linux {
    use std::{
        ffi::{CStr, c_char, c_int},
        fs::{self, File, OpenOptions},
        io,
        os::{
            fd::AsRawFd,
            unix::{ffi::OsStrExt, fs::symlink},
        },
        path::{Path, PathBuf},
    };

    unsafe extern "C" {
        fn grantpt(fd: c_int) -> c_int;
        fn unlockpt(fd: c_int) -> c_int;
        fn ptsname_r(fd: c_int, name: *mut c_char, len: usize) -> c_int;
    }

    pub struct HostPty {
        _master: File,
        link: PathBuf,
        slave: PathBuf,
    }

    impl HostPty {
        pub fn open(link: &Path, force: bool) -> io::Result<Self> {
            let master = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/ptmx")?;
            let fd = master.as_raw_fd();
            // SAFETY: The descriptor belongs to the open PTY master and the name
            // buffer is writable for the supplied length.
            let mut name = [0; 128];
            unsafe {
                if grantpt(fd) != 0 || unlockpt(fd) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let result = ptsname_r(fd, name.as_mut_ptr(), name.len());
                if result != 0 {
                    return Err(io::Error::from_raw_os_error(result));
                }
            }
            // SAFETY: A successful ptsname_r writes a NUL-terminated slave name.
            let slave = unsafe { CStr::from_ptr(name.as_ptr()) };
            let slave = PathBuf::from(std::ffi::OsStr::from_bytes(slave.to_bytes()));
            if let Err(error) = symlink(&slave, link) {
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
                if !force {
                    return Err(io::Error::new(
                        error.kind(),
                        "terminal link already exists; use forceCreate to overwrite it",
                    ));
                }
                let kind = fs::symlink_metadata(link)?.file_type();
                if !(kind.is_file() || kind.is_symlink()) {
                    return Err(error);
                }
                fs::remove_file(link)?;
                symlink(&slave, link)?;
            }
            Ok(Self {
                _master: master,
                link: link.to_owned(),
                slave,
            })
        }
    }

    impl Drop for HostPty {
        fn drop(&mut self) {
            if fs::read_link(&self.link).is_ok_and(|path| path == self.slave) {
                let _ = fs::remove_file(&self.link);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::FileTypeExt;

        #[test]
        fn renode_host_pty_force_replaces_temporary_file() {
            let dir = std::env::temp_dir().join(format!("penv-host-pty-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let link = dir.join("tty");
            fs::write(&link, b"placeholder").unwrap();
            assert!(
                matches!(HostPty::open(&link, false), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
            );
            assert_eq!(fs::read(&link).unwrap(), b"placeholder");
            {
                let terminal = HostPty::open(&link, true).unwrap();
                assert_eq!(fs::read_link(&link).unwrap(), terminal.slave);
                assert!(fs::metadata(&link).unwrap().file_type().is_char_device());
                File::open(&link).unwrap();
            }
            assert!(!link.exists());
            fs::remove_dir(dir).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::HostPty;

#[cfg(not(target_os = "linux"))]
pub struct HostPty;

#[cfg(not(target_os = "linux"))]
impl HostPty {
    pub fn open(_: &std::path::Path, _: bool) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "host PTY terminals require Linux",
        ))
    }
}
