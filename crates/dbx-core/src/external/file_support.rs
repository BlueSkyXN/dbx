use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::ExternalTableError;

pub(crate) fn file_sha256(path: &Path) -> Result<String, ExternalTableError> {
    let mut file = File::open(path)
        .map_err(|error| ExternalTableError::io(format!("Failed to open {} for hashing: {error}", path.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ExternalTableError::io(format!("Failed to hash {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn parse_index_key(key: &str, prefix: &str) -> Result<usize, ExternalTableError> {
    key.strip_prefix(prefix)
        .ok_or_else(|| ExternalTableError::invalid(format!("Invalid stable key: {key}")))?
        .parse::<usize>()
        .map_err(|_| ExternalTableError::invalid(format!("Invalid stable key: {key}")))
}

pub(crate) fn unique_display_names(raw: &[String]) -> Vec<String> {
    let mut used = std::collections::HashMap::<String, usize>::new();
    raw.iter()
        .enumerate()
        .map(|(index, value)| {
            let base = if value.trim().is_empty() { format!("column_{}", index + 1) } else { value.trim().to_string() };
            let count = used.entry(base.clone()).and_modify(|count| *count += 1).or_insert(1);
            if *count == 1 {
                base
            } else {
                format!("{base} ({count})")
            }
        })
        .collect()
}

pub(crate) fn replace_staged_file(staged: &Path, destination: &Path) -> Result<(), ExternalTableError> {
    #[cfg(unix)]
    {
        std::fs::rename(staged, destination).map_err(|error| {
            ExternalTableError::io(format!("Failed to atomically replace {}: {error}", destination.display()))
        })?;
        if let Some(parent) = destination.parent() {
            File::open(parent).and_then(|directory| directory.sync_all()).map_err(|error| {
                ExternalTableError::io(format!("Failed to sync directory {} after replace: {error}", parent.display()))
            })?;
        }
        return Ok(());
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_IGNORE_MERGE_ERRORS};

        let destination_wide = destination.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let staged_wide = staged.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let replaced = unsafe {
            ReplaceFileW(
                destination_wide.as_ptr(),
                staged_wide.as_ptr(),
                std::ptr::null(),
                REPLACE_FILE_IGNORE_MERGE_ERRORS,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if replaced == 0 {
            return Err(ExternalTableError::io(format!(
                "Failed to atomically replace {}: {}",
                destination.display(),
                std::io::Error::last_os_error()
            )));
        }
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(ExternalTableError::unsupported("Atomic external table file replacement is not supported on this platform"))
}
