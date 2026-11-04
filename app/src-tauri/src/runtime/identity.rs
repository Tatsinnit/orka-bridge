use super::types::RuntimeError;
use serde::{Deserialize, Serialize};
use std::fs::File;

/// Continuity hint, not durable creation authority: IDs may be reused after close.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    volume: u64,
    index: [u8; 16],
}
#[cfg(unix)]
pub fn file_identity(file: &File) -> Result<FileIdentity, RuntimeError> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    let mut index = [0; 16];
    index[..8].copy_from_slice(&m.ino().to_le_bytes());
    Ok(FileIdentity {
        volume: m.dev(),
        index,
    })
}
#[cfg(windows)]
pub fn file_identity(file: &File) -> Result<FileIdentity, RuntimeError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    // FILE_ID_INFO preserves the full 128-bit file identity, including on ReFS.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(FileIdentity {
        volume: info.VolumeSerialNumber,
        index: info.FileId.Identifier,
    })
}
