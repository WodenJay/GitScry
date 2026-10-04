//! Windows spill-file creation must not inherit a potentially shared temp-directory ACL.
use std::{
    fs::File,
    io,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL},
};

pub(super) fn create(path: &Path) -> io::Result<File> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    if path[..path.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "spill path contains NUL",
        ));
    }
    // A protected DACL grants full access only to the file's owner, without
    // inheriting any access granted by a shared temporary directory.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain([0]).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: strings are NUL-terminated, pointers remain valid throughout the
    // calls, and the allocated descriptor is freed after CreateFileW consumes it.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        );
        let result = if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            // The returned handle is exclusively owned and closed by File.
            Ok(File::from_raw_handle(handle))
        };
        LocalFree(descriptor);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION,
        },
    };

    #[test]
    fn spill_file_has_only_owner_access_even_under_an_inheriting_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let mut stdout = Vec::new();
        super::super::write_query(
            &"a".repeat(32769),
            &mut stdout,
            &mut Vec::new(),
            false,
            temporary.path(),
        )
        .unwrap();
        let stdout = String::from_utf8(stdout).unwrap();
        let path = stdout
            .lines()
            .find_map(|line| line.strip_prefix("Full output file: "))
            .unwrap();
        let path: Vec<u16> = Path::new(path)
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect();
        let mut descriptor = ptr::null_mut();
        let mut text = ptr::null_mut();
        let mut length = 0;
        // SAFETY: the path is NUL-terminated and all output pointers are valid. Both
        // APIs allocate their outputs with LocalAlloc, and we free them below.
        unsafe {
            assert_eq!(
                GetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    &mut length
                ),
                0
            );
            let wide = std::slice::from_raw_parts(text, length as usize);
            let end = wide.iter().position(|&unit| unit == 0).unwrap();
            let sddl = String::from_utf16(&wide[..end]).unwrap();
            LocalFree(text.cast());
            LocalFree(descriptor);
            assert_eq!(sddl, "D:P(A;;FA;;;OW)");
        }
    }
}
