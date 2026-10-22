//! The device key: this install's iroh identity, `device.key` in the data
//! directory (`docs/DESIGN.md` §3). Hot by design — it answers every
//! connection — and *not* the person's key, which lives in the keychain.

use std::{fs, io::Write, path::Path};

use hit_event::DeviceKey;

use crate::NodeError;

/// Loads `path`, or creates it with a fresh key readable by this user only.
pub(crate) fn load_or_create(path: &Path) -> Result<DeviceKey, NodeError> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                NodeError::DeviceKey(format!("{} is not a 32-byte key", path.display()))
            })?;
            Ok(DeviceKey::from_bytes(&bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = DeviceKey::generate();
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts
                .open(path)
                .map_err(|e| NodeError::DeviceKey(e.to_string()))?;
            file.write_all(&key.to_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| NodeError::DeviceKey(e.to_string()))?;
            Ok(key)
        }
        Err(e) => Err(NodeError::DeviceKey(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        let a = load_or_create(&path).unwrap();
        let b = load_or_create(&path).unwrap();
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_wrong_sized_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        fs::write(&path, b"short").unwrap();
        assert!(matches!(
            load_or_create(&path),
            Err(NodeError::DeviceKey(_))
        ));
    }
}
