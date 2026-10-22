//! The iroh endpoint and its key on disk.

use anyhow::{Context, Result};
use iroh::{endpoint::presets, Endpoint, SecretKey};
use std::path::Path;

pub const ALPN: &[u8] = b"hit/nostr/1";

/// The device key. A throwaway file in a throwaway spike; the product keeps
/// device keys as `device.key` and the person's key in the OS keychain.
fn load_or_create_key(dir: &Path) -> Result<SecretKey> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("iroh.key");
    if path.exists() {
        let bytes = hex::decode(std::fs::read_to_string(&path)?.trim())?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("bad key file"))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    std::fs::write(&path, hex::encode(key.to_bytes()))?;
    Ok(key)
}

pub async fn bind(dir: &Path, accept: bool) -> Result<Endpoint> {
    let mut b = Endpoint::builder(presets::N0).secret_key(load_or_create_key(dir)?);
    if accept {
        b = b.alpns(vec![ALPN.to_vec()]);
    }
    b.bind().await.context("bind iroh endpoint")
}
