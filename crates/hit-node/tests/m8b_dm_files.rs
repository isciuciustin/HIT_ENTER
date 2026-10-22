//! M8b, on loopback: a file in a DM (NIP-17 kind 15) reaches its recipient
//! byte for byte, **no disk holds it in the clear** — not the sender's, not
//! the recipient's, whose stores keep ciphertext and decrypt on reading —
//! and a member of a group they share, who knows the hash, is refused it.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{path::Path, time::Duration};

use common::{eventually, loopback, loopback_addr, mesh, start};
use hit_event::{MemoryStore, Person};

/// 200 KB with a marker every 1000 bytes: if any of it is stored in the
/// clear, the scan finds the marker.
fn plain() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..200 {
        let mut chunk = format!("PLAINTEXT-MARKER-{i:04}-").into_bytes();
        chunk.resize(1000, b'.');
        out.extend(chunk);
    }
    out
}

fn holds_marker(dir: &Path) -> Option<String> {
    for entry in walk(dir) {
        let Ok(bytes) = std::fs::read(&entry) else {
            continue;
        };
        if bytes.windows(16).any(|w| w == b"PLAINTEXT-MARKER") {
            return Some(entry.display().to_string());
        }
    }
    None
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_file_arrives_whole_is_never_stored_in_the_clear_and_is_refused_to_others() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, carol) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let c = start(dirs[2].path(), &carol).await;
    mesh(&[&a, &b, &c]);
    let g = a
        .create_group("Three", &MemoryStore::default())
        .await
        .unwrap();
    let link = a.invite(&g, None, Some(loopback_addr(&a))).await.unwrap();
    b.join(&link).await.unwrap();
    c.join(&link).await.unwrap();
    a.sync_with(loopback(&b)).await.unwrap();
    a.sync_with(loopback(&c)).await.unwrap();

    // The file lives outside every data directory, in the clear, as a file
    // the person picked would.
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("scan.pdf");
    let original = plain();
    std::fs::write(&path, &original).unwrap();

    let sent = a.send_dm_file(&bob.public_key(), &path).await.unwrap();
    let file = sent.file.clone().unwrap();
    assert_eq!(
        (file.name.as_str(), file.mime.as_str()),
        ("scan.pdf", "application/pdf")
    );
    assert!(
        eventually(Duration::from_secs(10), || async {
            b.dms().await.unwrap().iter().any(|d| d.id == sent.id)
        })
        .await,
        "Bob never got the DM"
    );

    // Bob opens it: fetched (ciphertext) from Alice, decrypted, whole.
    assert_eq!(b.dm_file_bytes(&sent.id).await.unwrap(), original);
    // Alice reads her own copy the same way.
    assert_eq!(a.dm_file_bytes(&sent.id).await.unwrap(), original);

    // Carol shares a group with both and learns the hash somehow: neither
    // will hand her the bytes — it was granted to the two of them only.
    assert!(c.fetch_file_from(loopback(&a), &file.hash).await.is_err());
    assert!(c.fetch_file_from(loopback(&b), &file.hash).await.is_err());
    // She cannot open the DM it came in either, so she has no key.
    assert!(c.dm_file_bytes(&sent.id).await.is_err());

    // No disk holds the plaintext: every file of both data directories —
    // the stores, the blob stores, their WALs — after a clean shutdown.
    a.shutdown().await;
    b.shutdown().await;
    c.shutdown().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for d in &dirs[..2] {
        assert_eq!(
            holds_marker(d.path()),
            None,
            "plaintext stored in the clear"
        );
    }
    // The control: the scan does find the marker where it is.
    assert!(holds_marker(outside.path()).is_some());
}
