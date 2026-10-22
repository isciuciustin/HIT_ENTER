//! M1's "done when" (`docs/PLAN.md`), as tests.
//!
//! 1. Make a person, bind two devices, verify each binding against its own
//!    `EndpointId` and reject it against the other, revoke one, reject it
//!    after.
//! 2. Prove no nsec is written to a plain file: run everything a node would
//!    put on disk or in a log, write it all into a data directory, and scan
//!    the bytes for the secret in every spelling.
//!
//! Plus a real OS-keychain round trip, `#[ignore]`d because it needs a
//! running Secret Service / Keychain and writes (then deletes) one entry:
//!
//! ```text
//! cargo test -p hit-event --test m1_done_when -- --ignored
//! ```

// Every line here is test code, helpers included; clippy.toml's exemption
// covers only `#[test]` functions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{fs, path::Path, time::Duration};

use hit_event::{
    BindingBook, BindingError, DeviceKey, Inserted, MemoryStore, Person, SecretStore, Timestamp,
    sign_binding, sign_revocation, verify_binding_at,
};
use nostr::prelude::{FromBech32, SecretKey};

const DAY: Duration = Duration::from_secs(86_400);

#[test]
fn a_person_binds_two_devices_and_revokes_one() {
    let now = Timestamp::now();
    let person = Person::generate();
    let (laptop, pc) = (DeviceKey::generate(), DeviceKey::generate());
    let laptop_b = sign_binding(&person, &laptop, now, hit_event::binding::DEFAULT_TTL).unwrap();
    let pc_b = sign_binding(&person, &pc, now, hit_event::binding::DEFAULT_TTL).unwrap();

    // Each verifies against its own device, and is refused against the other.
    assert_eq!(
        verify_binding_at(&laptop_b, laptop.public(), now),
        Ok(person.public_key())
    );
    assert_eq!(
        verify_binding_at(&pc_b, pc.public(), now),
        Ok(person.public_key())
    );
    assert_eq!(
        verify_binding_at(&laptop_b, pc.public(), now),
        Err(BindingError::WrongDevice)
    );
    assert_eq!(
        verify_binding_at(&pc_b, laptop.public(), now),
        Err(BindingError::WrongDevice)
    );

    // A peer's book, authenticating connections the way a node will.
    let mut book = BindingBook::new();
    assert_eq!(
        book.authenticate(&laptop_b, laptop.public(), now),
        Ok(person.public_key())
    );
    assert_eq!(
        book.authenticate(&pc_b, pc.public(), now),
        Ok(person.public_key())
    );

    // The laptop is lost. The person revokes it from the PC, a second later.
    let later = now + Duration::from_secs(1);
    let revocation = sign_revocation(&person, laptop.public(), later).unwrap();
    assert_eq!(book.insert(revocation, later), Ok(Inserted::Stored));

    // The laptop presenting its old, still-unexpired binding is refused…
    assert_eq!(
        book.authenticate(&laptop_b, laptop.public(), later),
        Err(BindingError::Revoked)
    );
    // …the PC is unaffected…
    assert_eq!(
        book.authenticate(&pc_b, pc.public(), later),
        Ok(person.public_key())
    );
    // …and discovery offers only the PC.
    assert_eq!(
        book.devices_of(&person.public_key(), later),
        vec![pc.public()]
    );

    // Expiry is the backstop for a peer that never hears the revocation.
    assert_eq!(
        verify_binding_at(&laptop_b, laptop.public(), now + 8 * DAY),
        Err(BindingError::Expired)
    );
}

#[test]
fn no_nsec_is_written_to_a_plain_file() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path();
    let store = MemoryStore::default();

    // Onboarding, as the app will do it.
    let person = Person::generate();
    let nsec = person.reveal_nsec_for_backup(); // shown once, on screen — not written
    person.save(&store).unwrap();
    let backup = person.to_ncryptsec("a long passphrase").unwrap();
    let device = DeviceKey::generate();
    let now = Timestamp::now();
    let binding = sign_binding(&person, &device, now, hit_event::binding::DEFAULT_TTL).unwrap();
    let revocation = sign_revocation(&person, device.public(), now + DAY).unwrap();

    // Everything that legitimately reaches disk: events, the encrypted backup,
    // and what a log line would say about a person and about every failure.
    let mut log = String::new();
    log += &format!("{person:?}\n{}\n", person.npub());
    log += &format!(
        "{:?}\n",
        Person::from_ncryptsec(&backup, "wrong password").unwrap_err()
    );
    log += &format!(
        "{}\n",
        Person::from_ncryptsec(&backup, "wrong password").unwrap_err()
    );
    log += &format!(
        "{:?}\n",
        Person::from_nsec(&nsec[..nsec.len() - 1]).unwrap_err()
    );
    store.put(&person.npub(), "not a key").unwrap();
    log += &format!("{:?}\n", Person::load(&store, &person.npub()).unwrap_err());
    log += &format!(
        "{:?}\n",
        verify_binding_at(&binding, DeviceKey::generate().public(), now)
    );
    fs::write(
        dir.join("events.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::to_string(&binding).unwrap(),
            serde_json::to_string(&revocation).unwrap()
        ),
    )
    .unwrap();
    fs::write(dir.join("backup.ncryptsec"), &backup).unwrap();
    fs::write(dir.join("node.log"), &log).unwrap();

    let secret = SecretKey::from_bech32(nsec.as_str()).unwrap();
    let needles: Vec<Vec<u8>> = vec![
        nsec.as_bytes().to_vec(),
        secret.to_secret_hex().into_bytes(),
        secret.to_secret_hex().to_uppercase().into_bytes(),
        secret.to_secret_bytes().to_vec(),
    ];

    // The scan must be able to see a leak, or its silence means nothing.
    let control = tempfile::tempdir().unwrap();
    fs::write(control.path().join("oops.txt"), nsec.as_bytes()).unwrap();
    assert!(
        !scan(control.path(), &needles).is_empty(),
        "the scan cannot see a planted nsec"
    );

    assert_eq!(
        scan(dir, &needles),
        Vec::<String>::new(),
        "a secret reached the data dir"
    );
}

/// Every file under `dir` containing any needle.
fn scan(dir: &Path, needles: &[Vec<u8>]) -> Vec<String> {
    let mut hits = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            hits.extend(scan(&path, needles));
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        if needles
            .iter()
            .any(|n| bytes.windows(n.len()).any(|w| w == n.as_slice()))
        {
            hits.push(path.display().to_string());
        }
    }
    hits
}

#[test]
#[ignore = "needs the OS keychain; writes and deletes one throwaway entry"]
fn the_os_keychain_round_trips_a_throwaway_identity() {
    let service = format!("hit-enter-test-{}", std::process::id());
    let keychain = hit_event::Keychain::new(&service).unwrap();
    let person = Person::generate();
    person.save(&keychain).unwrap();
    let loaded = Person::load(&keychain, &person.npub()).unwrap().unwrap();
    assert_eq!(loaded.public_key(), person.public_key());
    keychain.delete(&person.npub()).unwrap();
    assert!(Person::load(&keychain, &person.npub()).unwrap().is_none());
}
