use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path};
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    NtCreateFile, NtFlushBuffersFile, RtlNtStatusToDosErrorNoTeb, FILE_CREATE, FILE_DIRECTORY_FILE,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    HANDLE, OBJ_CASE_INSENSITIVE, STATUS_OBJECT_NAME_NOT_FOUND, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileDispositionInfoEx, FileRenameInfoEx, SetFileInformationByHandle, DELETE,
    FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_RENAME_INFO,
    FILE_RENAME_INFO_0, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};

const REPARSE_POINT_ATTRIBUTE: u32 = 0x0000_0400;

pub(crate) struct ParentHandle {
    pub(crate) directory: File,
    pub(crate) file_name: OsString,
}

pub(crate) fn open_root(root: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(root)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT_ATTRIBUTE != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed root is not a non-reparse directory",
        ));
    }
    Ok(file)
}

pub(crate) fn open_parent(
    root: &Path,
    relative: &str,
    create: bool,
) -> io::Result<Option<ParentHandle>> {
    let mut directory = open_root(root)?;
    let components = Path::new(relative).components().collect::<Vec<_>>();
    let Some((file_component, parents)) = components.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty managed path",
        ));
    };
    for component in parents {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "non-normal managed path",
            ));
        };
        directory = match nt_open_relative(
            &directory,
            name,
            FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            if create { FILE_OPEN_IF } else { FILE_OPEN },
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        ) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) => return Err(error),
        };
        directory = ensure_non_reparse_directory(directory)?;
    }
    let Component::Normal(file_name) = file_component else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "non-normal managed file",
        ));
    };
    Ok(Some(ParentHandle {
        directory,
        file_name: file_name.to_os_string(),
    }))
}

pub(crate) fn read_optional(parent: &ParentHandle) -> io::Result<Option<Vec<u8>>> {
    read_named_optional(&parent.directory, &parent.file_name)
}

pub(crate) fn read_named_optional(parent: &File, name: &OsStr) -> io::Result<Option<Vec<u8>>> {
    let mut file = match open_regular(parent, name, FILE_OPEN, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    read_all(&mut file).map(Some)
}

pub(crate) fn read_all(file: &mut File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn create_regular(parent: &ParentHandle) -> io::Result<File> {
    create_named(&parent.directory, &parent.file_name)
}

pub(crate) fn create_named(parent: &File, name: &OsStr) -> io::Result<File> {
    open_regular(parent, name, FILE_CREATE, true)
}

pub(crate) fn open_regular_existing(parent: &ParentHandle, delete: bool) -> io::Result<File> {
    open_named_existing(&parent.directory, &parent.file_name, delete)
}

pub(crate) fn open_regular_existing_for_metadata(parent: &ParentHandle) -> io::Result<File> {
    nt_open_relative(
        &parent.directory,
        &parent.file_name,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
    )
    .and_then(ensure_non_reparse_file)
}

pub(crate) fn open_named_existing(parent: &File, name: &OsStr, delete: bool) -> io::Result<File> {
    let access = FILE_READ_DATA
        | FILE_READ_ATTRIBUTES
        | SYNCHRONIZE
        | if delete {
            DELETE | FILE_WRITE_ATTRIBUTES
        } else {
            0
        };
    nt_open_relative(
        parent,
        name,
        access,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
    )
    .and_then(ensure_non_reparse_file)
}

pub(crate) fn remove_directory(parent: &ParentHandle) -> io::Result<()> {
    let directory = nt_open_relative(
        &parent.directory,
        &parent.file_name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
        FILE_OPEN,
        FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
    )
    .and_then(ensure_non_reparse_directory)?;
    delete_on_close(&directory)?;
    flush(&parent.directory)
}

pub(crate) fn rename_to(file: &File, parent: &File, name: &OsStr) -> io::Result<()> {
    let encoded = encode_component(name)?;
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let byte_size = offset + encoded.len() * size_of::<u16>();
    let word_count = byte_size.div_ceil(size_of::<usize>());
    // FILE_RENAME_INFO contains pointer-sized fields and must not be placed in a byte-aligned Vec.
    let mut buffer = vec![0_usize; word_count];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFO_0 { Flags: 0 };
        (*info).RootDirectory = parent.as_raw_handle() as HANDLE;
        (*info).FileNameLength = (encoded.len() * size_of::<u16>()) as u32;
        ptr::copy_nonoverlapping(
            encoded.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(offset).cast(),
            encoded.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileRenameInfoEx,
            buffer.as_ptr().cast(),
            byte_size as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    flush(parent)
}

pub(crate) fn delete_on_close(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfoEx,
            (&info as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn same_directory(left: &File, right: &File) -> io::Result<bool> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.volume_serial_number() == right.volume_serial_number()
        && left.file_index() == right.file_index())
}

fn open_regular(parent: &File, name: &OsStr, disposition: u32, write: bool) -> io::Result<File> {
    let access = FILE_READ_DATA
        | FILE_READ_ATTRIBUTES
        | SYNCHRONIZE
        | if write {
            FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | DELETE
        } else {
            0
        };
    nt_open_relative(
        parent,
        name,
        access,
        disposition,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
    )
    .and_then(ensure_non_reparse_file)
}

fn nt_open_relative(
    parent: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    share: u32,
) -> io::Result<File> {
    let mut encoded = encode_component(name)?;
    let mut unicode = UNICODE_STRING {
        Length: (encoded.len() * size_of::<u16>()) as u16,
        MaximumLength: (encoded.len() * size_of::<u16>()) as u16,
        Buffer: encoded.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle() as HANDLE,
        ObjectName: &mut unicode,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut handle: HANDLE = ptr::null_mut();
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status_block,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options,
            ptr::null(),
            0,
        )
    };
    if status < 0 {
        let code = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    if handle.is_null() || status == STATUS_OBJECT_NAME_NOT_FOUND {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "managed entry not found",
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle.cast()) })
}

fn ensure_non_reparse_directory(file: File) -> io::Result<File> {
    let metadata = file.metadata()?;
    if metadata.is_dir() && metadata.file_attributes() & REPARSE_POINT_ATTRIBUTE == 0 {
        Ok(file)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed parent is a reparse point",
        ))
    }
}

fn ensure_non_reparse_file(file: File) -> io::Result<File> {
    let metadata = file.metadata()?;
    if metadata.is_file() && metadata.file_attributes() & REPARSE_POINT_ATTRIBUTE == 0 {
        Ok(file)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed entry is not a regular file",
        ))
    }
}

pub(crate) fn write_synced(mut file: File, bytes: &[u8]) -> io::Result<File> {
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(file)
}

pub(crate) fn flush(file: &File) -> io::Result<()> {
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let status = unsafe { NtFlushBuffersFile(file.as_raw_handle() as HANDLE, &mut status_block) };
    if status < 0 {
        let code = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

fn encode_component(name: &OsStr) -> io::Result<Vec<u16>> {
    let encoded = name.encode_wide().collect::<Vec<_>>();
    if encoded.is_empty()
        || encoded.contains(&0)
        || encoded.iter().any(|unit| {
            *unit == u16::from(b'/') || *unit == u16::from(b'\\') || *unit == u16::from(b':')
        })
        || matches!(encoded.last(), Some(unit) if *unit == b'.' as u16 || *unit == b' ' as u16)
        || encoded.len().saturating_mul(size_of::<u16>()) > u16::MAX as usize
        || is_dos_device_name(&encoded)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Windows managed path component",
        ));
    }
    Ok(encoded)
}

fn is_dos_device_name(encoded: &[u16]) -> bool {
    let stem = encoded
        .split(|unit| *unit == b'.' as u16)
        .next()
        .unwrap_or_default();
    let upper = stem
        .iter()
        .map(|unit| {
            char::from_u32(u32::from(*unit))
                .unwrap_or('\0')
                .to_ascii_uppercase()
        })
        .collect::<String>();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && matches!(upper.as_bytes()[3], b'1'..=b'9'))
}
