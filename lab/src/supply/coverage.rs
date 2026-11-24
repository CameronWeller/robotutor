//! Exposure to one reviewed task across a known set of delivery channels.
use super::Contact;
use std::{fs::OpenOptions, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskContact {
    /// Every expected channel in this read covered the interval and recorded no delivery.
    NoRecordedContact,
    Unknown,
    Possible,
    Confirmed,
}

/// Admits at most one attempt to show this semantic task across cooperating
/// channels for one learner. The host supplies an existing, access-controlled
/// directory for that person, a stable catalog task ID (e.g. Form::task), and
/// current authorized coverage. All channels must use the same directory.
///
/// Requires atomic exclusive creation and durable directory sync on local
/// storage. A storage error stops delivery. Keep the marker through errors,
/// restarts and timeouts: losing a receipt cannot prove that no bytes arrived.
/// A returned Some contains the delivery's own result, including any error.
pub fn open_once<T>(
    learner_directory: &Path,
    task: u64,
    coverage: TaskContact,
    deliver: impl FnOnce() -> T,
) -> io::Result<Option<T>> {
    if coverage != TaskContact::NoRecordedContact {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH;
        options.custom_flags(FILE_FLAG_WRITE_THROUGH);
    }
    let path = learner_directory.join(format!("{task:016x}"));
    let marker = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(error),
    };
    marker.sync_all()?;
    crate::platform::sync_created_entry(&path)?;
    Ok(Some(deliver()))
}

impl TaskContact {
    pub fn on_channel(contact: Option<Contact>, complete: bool) -> Self {
        match contact {
            Some(Contact::Delivered) => Self::Confirmed,
            Some(Contact::Possible) => Self::Possible,
            None if complete => Self::NoRecordedContact,
            None => Self::Unknown,
        }
    }

    /// The host resolves identity, task equivalence, authorization and the
    /// required interval *before* reading a source. Include Unknown for an
    /// uncovered channel. One confirmed delivery suffices even with a gap.
    pub fn across(channels: impl IntoIterator<Item = Self>) -> Self {
        channels
            .into_iter()
            .reduce(|left, right| match (left, right) {
                (Self::Confirmed, _) | (_, Self::Confirmed) => Self::Confirmed,
                (Self::Possible, _) | (_, Self::Possible) => Self::Possible,
                (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
                (Self::NoRecordedContact, Self::NoRecordedContact) => Self::NoRecordedContact,
            })
            .unwrap_or(Self::Unknown)
    }
}
