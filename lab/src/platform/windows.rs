use std::{
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{addr_of, null_mut},
};
use windows_sys::Win32::{
    Foundation::{LocalFree, HANDLE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT,
        },
        *,
    },
    Storage::FileSystem::{
        CreateDirectoryW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    unsafe {
        let mut info = zeroed();
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }
}

pub fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    let i = info(file)?;
    Ok((
        i.dwVolumeSerialNumber.into(),
        (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow),
    ))
}

fn token_user() -> io::Result<Vec<usize>> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut size = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut user = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            user.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(user)
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

// Examine the opened object, including its owner and every access-granting ACE.
// Unexpected ACE formats fail closed. Administrators and SYSTEM already control this PC.
pub fn private_handle(file: &File) -> io::Result<bool> {
    let i = info(file)?;
    if i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (i.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && i.nNumberOfLinks != 1)
    {
        return Ok(false);
    }
    unsafe {
        let user = token_user()?;
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let trusted = |other| {
            EqualSid(sid, other) != 0
                || IsWellKnownSid(other, WinLocalSystemSid) != 0
                || IsWellKnownSid(other, WinBuiltinAdministratorsSid) != 0
        };
        let (mut owner, mut acl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        let err = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        );
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        let _owned = Descriptor(descriptor);
        if owner.is_null() || acl.is_null() || !trusted(owner) {
            return Ok(false);
        }
        for index in 0..(*acl).AceCount {
            let mut ptr = null_mut();
            if GetAce(acl, index.into(), &mut ptr) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = &*ptr.cast::<ACE_HEADER>();
            if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 || header.AceType == 1 {
                continue;
            }
            if header.AceType != 0 {
                return Ok(false);
            }
            let ace = &*ptr.cast::<ACCESS_ALLOWED_ACE>();
            if ace.Mask != 0 && !trusted(addr_of!(ace.SidStart).cast_mut().cast()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub fn create_directory(path: &std::path::Path) -> io::Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    unsafe {
        let user = token_user()?;
        let mut text = null_mut();
        if ConvertSidToStringSidW((*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid, &mut text) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _text = Descriptor(text.cast());
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        let sddl = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(OsStr::new(&sddl)).as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _owned = Descriptor(descriptor);
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        if CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attrs) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
