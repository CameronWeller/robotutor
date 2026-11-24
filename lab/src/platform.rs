//! OS-specific storage and process operations used by the tutor.
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
    process::Command,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{file_identity, private_handle};

#[cfg(unix)]
pub fn private_handle(file: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok(m.mode() & 0o077 == 0 && (m.is_dir() || (m.is_file() && m.nlink() == 1)))
}
#[cfg(unix)]
pub fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}

pub trait PrivateOpen {
    fn open_private(&mut self, path: impl AsRef<Path>) -> io::Result<File>;
}
impl PrivateOpen for OpenOptions {
    fn open_private(&mut self, path: impl AsRef<Path>) -> io::Result<File> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            self.mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            self.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = self.open(path)?;
        if !file.metadata()?.is_file() || !private_handle(&file)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Expected a private regular file",
            ));
        }
        Ok(file)
    }
}

pub fn private_directory(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path)?;
        Ok(meta.is_dir() && meta.mode() & 0o077 == 0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        Ok(dir.metadata()?.is_dir() && private_handle(&dir)?)
    }
}

pub fn create_private_dir(path: &Path, recursive: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(recursive)
            .create(path)?;
    }
    #[cfg(windows)]
    {
        if recursive {
            if let Some(parent) = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty() && !p.exists())
            {
                create_private_dir(parent, true)?;
            }
        }
        match windows::create_directory(path) {
            Err(e) if recursive && e.kind() == io::ErrorKind::AlreadyExists => (),
            result => result?,
        }
    }
    if !private_directory(path)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Select a private learner directory",
        ));
    }
    Ok(())
}

pub trait BackgroundCommand {
    fn background(&mut self) -> &mut Self;
}
impl BackgroundCommand for Command {
    fn background(&mut self) -> &mut Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            self.process_group(0)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{
                CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
            };
            self.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
        }
    }
}

pub fn python() -> std::ffi::OsString {
    std::env::var_os("ROBOTUTOR_PYTHON").unwrap_or_else(|| {
        if cfg!(target_os = "linux") {
            "/usr/bin/python3"
        } else if cfg!(windows) {
            "python"
        } else {
            "python3"
        }
        .into()
    })
}

/// Unix persists a new directory entry separately from its contents. On Windows
/// the creator uses write-through and flushes the writable file handle.
pub fn sync_created_entry(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Commit an already flushed staging file within its destination directory.
pub fn replace_staged(directory: &Path, staged: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let dir = File::open(directory)?;
        std::fs::rename(staged, destination)?;
        dir.sync_all()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let _ = directory;
        fn wide(path: &Path) -> io::Result<Vec<u16>> {
            let mut words: Vec<_> = path.as_os_str().encode_wide().collect();
            if words.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "NUL in file path",
                ));
            }
            words.push(0);
            Ok(words)
        }
        let from = wide(staged)?;
        let to = wide(destination)?;
        // Both UTF-16 buffers remain alive through the synchronous Windows call.
        let ok = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::PermissionDenied {
                return Err(error);
            }
            // Rust retries the native rename with POSIX sharing semantics on
            // supported Windows filesystems, preserving already-open readers.
            std::fs::rename(staged, destination)?;
            OpenOptions::new().write(true).open(destination)?.sync_all()?;
        }
        Ok(())
    }
}

pub fn project_root() -> std::path::PathBuf {
    crate::installation::project_root(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
}
