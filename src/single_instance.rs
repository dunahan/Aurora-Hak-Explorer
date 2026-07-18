//! Routes archive paths from later AHE launches to the already-running window.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

pub enum Launch {
    Forwarded,
    Primary(mpsc::Receiver<Vec<PathBuf>>),
}

pub fn route(arguments: &[PathBuf]) -> Launch {
    let endpoint = endpoint_path();
    if forward(&endpoint, arguments).is_ok() {
        return Launch::Forwarded;
    }

    #[cfg(unix)]
    let _ = std::fs::remove_file(&endpoint);

    let listener = match UnixListener::bind(&endpoint) {
        Ok(listener) => listener,
        Err(_) => {
            // Another process may have won the startup race after our first
            // connection attempt. Give forwarding one final chance.
            if forward(&endpoint, arguments).is_ok() {
                return Launch::Forwarded;
            }
            let (_sender, receiver) = mpsc::channel();
            return Launch::Primary(receiver);
        }
    };

    #[cfg(unix)]
    set_owner_only_permissions(&endpoint);

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else {
                continue;
            };
            if let Ok(paths) = read_paths(&mut stream)
                && sender.send(paths).is_err()
            {
                break;
            }
        }
    });
    Launch::Primary(receiver)
}

fn endpoint_path() -> PathBuf {
    #[cfg(unix)]
    {
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        directory.join(format!("aurora-hak-explorer-{}.sock", unsafe {
            libc::geteuid()
        }))
    }
    #[cfg(windows)]
    {
        std::env::temp_dir().join("aurora-hak-explorer.sock")
    }
}

fn forward(endpoint: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let mut stream = UnixStream::connect(endpoint)?;
    write_paths(&mut stream, paths)
}

fn write_paths(writer: &mut impl Write, paths: &[PathBuf]) -> io::Result<()> {
    let count = u32::try_from(paths.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many paths"))?;
    writer.write_all(&count.to_le_bytes())?;
    for path in paths {
        let bytes = os_string_bytes(path.as_os_str());
        let length = u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path is too long"))?;
        writer.write_all(&length.to_le_bytes())?;
        writer.write_all(&bytes)?;
    }
    writer.flush()
}

fn read_paths(reader: &mut impl Read) -> io::Result<Vec<PathBuf>> {
    const MAX_PATHS: usize = 1_024;
    const MAX_PATH_BYTES: usize = 1024 * 1024;

    let count = read_u32(reader)? as usize;
    if count > MAX_PATHS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many forwarded paths",
        ));
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let length = read_u32(reader)? as usize;
        if length > MAX_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "forwarded path is too long",
            ));
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        paths.push(PathBuf::from(bytes_os_string(bytes)?));
    }
    Ok(paths)
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(unix)]
fn os_string_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(unix)]
fn bytes_os_string(bytes: Vec<u8>) -> io::Result<OsString> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(bytes))
}

#[cfg(windows)]
fn os_string_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(windows)]
fn bytes_os_string(bytes: Vec<u8>) -> io::Result<OsString> {
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid UTF-16 path bytes",
        ));
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    Ok(OsString::from_wide(&wide))
}

#[cfg(unix)]
fn set_owner_only_permissions(endpoint: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_messages_round_trip() {
        let expected = vec![
            PathBuf::from("/tmp/first archive.hak"),
            PathBuf::from("/tmp/second.hak"),
        ];
        let mut message = Vec::new();
        write_paths(&mut message, &expected).unwrap();
        assert_eq!(read_paths(&mut message.as_slice()).unwrap(), expected);
    }

    #[test]
    fn path_message_rejects_excessive_count() {
        let message = 1_025_u32.to_le_bytes();
        assert!(read_paths(&mut message.as_slice()).is_err());
    }
}
