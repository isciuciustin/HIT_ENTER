//! Direct messages: NIP-17, as specified (M7, `DESIGN.md` §7).
//!
//! A message is a kind 14 **rumor** (unsigned), **sealed** (kind 13) to the
//! recipient with NIP-44 and signed by the sender, then **gift-wrapped**
//! (kind 1059) to the recipient with NIP-44 by a throwaway key. Only the
//! recipient can open it; anyone holding it sees a random author, the
//! recipient's key in `p`, a timestamp NIP-59 blurs up to two days, and
//! ciphertext. The sender wraps a second copy to themselves, so their own
//! other devices — and their history — have it too.
//!
//! **None of the cryptography is ours**: the `nostr` crate's NIP-44 and
//! NIP-59 do it. This module chooses the shape and keeps the person's secret
//! inside [`Person`].
//!
//! **The sender's device binding rides inside the rumor** (`["device",
//! <kind 30436 as JSON>]`, M9): sealed, so only the recipient sees it. It is
//! how two people with no group in common keep each other reachable — the
//! recipient learns which devices to hand a reply to, and to ring for a
//! call. Other Nostr clients ignore the tag.
//!
//! Where a wrap is kept, and who may be handed it, is `hit-group`'s: its
//! recipient, and the anchors the recipient names in their **inbox list**
//! (kind 10050, below).

use std::str::FromStr;

use iroh_base::EndpointId;
use nostr::prelude::{
    Event, EventBuilder, FinalizeEvent, FinalizeUnsignedEvent, Kind, PublicKey, Tag, Timestamp,
    UnsignedEvent,
    nip59::{GiftWrapBuilder, UnwrappedGift},
};

use crate::{KeyError, Person};

/// NIP-17's chat message — the rumor inside, never published as itself.
pub const DM: Kind = Kind::from_u16(14);
/// NIP-17's file message (M8b): the file encrypted, its key in here.
pub const DM_FILE: Kind = Kind::from_u16(15);
/// The cipher NIP-17 names for a file, and the only one we write or read.
pub const FILE_CIPHER: &str = "aes-gcm";
/// NIP-59's gift wrap: the only DM kind a node stores or sends.
pub const GIFT_WRAP: Kind = Kind::from_u16(1059);
/// NIP-17's DM relay list — here, a person's **inbox list**: the anchors
/// that keep their DMs while they are away (M7).
pub const INBOX_LIST: Kind = Kind::from_u16(10050);

/// The rumor tag carrying the sender's device binding.
pub const DEVICE_TAG: &str = "device";

/// The longest DM, in characters: a message, as in a group.
pub const MAX_DM_CHARS: usize = 4000;

/// A DM, opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dm {
    /// The rumor's id: the message's identity, the same in every wrap of it.
    pub id: String,
    pub from: PublicKey,
    pub to: PublicKey,
    pub text: String,
    /// The real send time — inside the seal, where only the two can see it.
    pub at: Timestamp,
    /// A file, if this is a kind 15 (M8b). `text` is then empty.
    pub file: Option<DmFile>,
    /// The sender's device binding, if the rumor carried one **of the
    /// sender's** — unchecked beyond that; the binding book checks it.
    pub device: Option<Event>,
}

/// A file sent in a DM (NIP-17 kind 15, M8b). The bytes stored and sent are
/// **ciphertext** (AES-256-GCM); `key` and `nonce` live only in the sealed
/// rumor, so only the two people can open what any node holds.
#[derive(Clone, PartialEq, Eq)]
pub struct DmFile {
    /// BLAKE3 of the ciphertext: what iroh fetches and verifies.
    pub hash: String,
    /// SHA-256 of the ciphertext (NIP-17's `x`).
    pub sha256: String,
    /// SHA-256 of the file before encryption (NIP-17's `ox`), checked
    /// after decrypting.
    pub original_sha256: String,
    /// Bytes of the original file.
    pub size: u64,
    pub mime: String,
    pub name: String,
    /// 32 bytes, hex.
    pub key: String,
    /// 12 bytes, hex.
    pub nonce: String,
}

impl std::fmt::Debug for DmFile {
    /// Everything but the key: a log line must not open the file.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DmFile")
            .field("hash", &self.hash)
            .field("size", &self.size)
            .field("mime", &self.mime)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Encrypts a file for a DM: a fresh key and nonce, AES-256-GCM (RustCrypto's
/// `aes-gcm`, not ours). Returns `(ciphertext, key hex, nonce hex, sha256 of
/// the plaintext hex, sha256 of the ciphertext hex)`.
pub fn seal_file(plain: &[u8]) -> Result<(Vec<u8>, String, String, String, String), KeyError> {
    use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
    use sha2::Digest;
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    let mut nonce = [0u8; 12];
    getrandom::fill(key.as_mut()).map_err(|_| KeyError::Sign)?;
    getrandom::fill(&mut nonce).map_err(|_| KeyError::Sign)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| KeyError::Sign)?;
    let sealed = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain)
        .map_err(|_| KeyError::Sign)?;
    Ok((
        sealed.clone(),
        hex::encode(key.as_ref()),
        hex::encode(nonce),
        hex::encode(sha2::Sha256::digest(plain)),
        hex::encode(sha2::Sha256::digest(&sealed)),
    ))
}

/// Opens a DM file's ciphertext with its key; `None` if the key, the nonce
/// or the bytes are wrong, or the result is not the file the sender hashed.
pub fn open_file(file: &DmFile, sealed: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
    use sha2::Digest;
    let key = zeroize::Zeroizing::new(hex::decode(&file.key).ok()?);
    let nonce = hex::decode(&file.nonce).ok()?;
    if key.len() != 32 || nonce.len() != 12 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let plain = cipher
        .decrypt(aes_gcm::Nonce::from_slice(&nonce), sealed)
        .ok()?;
    (hex::encode(sha2::Sha256::digest(&plain)) == file.original_sha256).then_some(plain)
}

impl Person {
    /// Wraps `text` to `to`, and again to this person: `(for them, for me)`.
    /// One rumor, two wraps, as NIP-17 says.
    /// `device`, this device's binding, rides along sealed.
    pub fn wrap_dm(
        &self,
        to: &PublicKey,
        text: &str,
        device: Option<&Event>,
    ) -> Result<(Event, Event), KeyError> {
        let rumor: UnsignedEvent = with_device(EventBuilder::new(DM, text), device)
            .tag(Tag::public_key(*to))
            .finalize_unsigned(self.public_key());
        self.wrap_rumor(to, rumor)
    }

    /// Wraps a file message (NIP-17 kind 15) to `to` and to this person.
    pub fn wrap_dm_file(
        &self,
        to: &PublicKey,
        file: &DmFile,
        device: Option<&Event>,
    ) -> Result<(Event, Event), KeyError> {
        let row = |n: &str, v: &str| Tag::custom(n, [v.to_owned()]);
        let rumor: UnsignedEvent = with_device(
            EventBuilder::new(DM_FILE, format!("{}{}", crate::group::BLOB_URL, file.hash)),
            device,
        )
        .tag(Tag::public_key(*to))
        .tag(row("file-type", &file.mime))
        .tag(row("encryption-algorithm", FILE_CIPHER))
        .tag(row("decryption-key", &file.key))
        .tag(row("decryption-nonce", &file.nonce))
        .tag(row("x", &file.sha256))
        .tag(row("ox", &file.original_sha256))
        .tag(row("size", &file.size.to_string()))
        .tag(row("name", &file.name))
        .finalize_unsigned(self.public_key());
        self.wrap_rumor(to, rumor)
    }

    fn wrap_rumor(&self, to: &PublicKey, rumor: UnsignedEvent) -> Result<(Event, Event), KeyError> {
        let theirs = GiftWrapBuilder::new(*to, rumor.clone())
            .finalize(self.keys())
            .map_err(|_| KeyError::Sign)?;
        let mine = GiftWrapBuilder::new(self.public_key(), rumor)
            .finalize(self.keys())
            .map_err(|_| KeyError::Sign)?;
        Ok((theirs, mine))
    }

    /// Opens a wrap addressed to this person. `None` if it is not one, not
    /// for us, or does not open — the seal's signer must be the rumor's
    /// author (checked by `nostr`), and the rumor a kind 14.
    pub fn open_dm(&self, wrap: &Event) -> Option<Dm> {
        if wrap.kind != GIFT_WRAP || recipient_of(wrap) != Some(self.public_key()) {
            return None;
        }
        let UnwrappedGift { sender, mut rumor } =
            UnwrappedGift::from_gift_wrap(self.keys(), wrap).ok()?;
        let to = rumor.tags.public_keys().next()?;
        let id = rumor.id().to_hex();
        let file = match rumor.kind {
            k if k == DM => None,
            k if k == DM_FILE => Some(parse_file(&rumor)?),
            _ => return None,
        };
        let device = rumor.tags.iter().find_map(|t| match t.as_slice() {
            [k, v, ..] if k == DEVICE_TAG => Event::from_json(v).ok(),
            _ => None,
        });
        Some(Dm {
            device: device.filter(|d| d.pubkey == sender),
            id,
            from: sender,
            to,
            text: if file.is_some() {
                String::new()
            } else {
                rumor.content
            },
            at: rumor.created_at,
            file,
        })
    }
}

fn with_device(b: EventBuilder, device: Option<&Event>) -> EventBuilder {
    match device {
        Some(d) => b.tag(Tag::custom(DEVICE_TAG, [d.as_json()])),
        None => b,
    }
}

/// A kind 15's file: our `hit-blob:` address, AES-GCM, every field there.
fn parse_file(rumor: &UnsignedEvent) -> Option<DmFile> {
    let tag = |n: &str| {
        rumor.tags.iter().find_map(|t| match t.as_slice() {
            [k, v, ..] if k == n => Some(v.clone()),
            _ => None,
        })
    };
    if tag("encryption-algorithm")?.as_str() != FILE_CIPHER {
        return None;
    }
    let f = DmFile {
        hash: rumor
            .content
            .strip_prefix(crate::group::BLOB_URL)?
            .to_owned(),
        sha256: tag("x")?,
        original_sha256: tag("ox")?,
        size: tag("size")?.parse().ok()?,
        mime: tag("file-type").unwrap_or_else(|| "application/octet-stream".into()),
        name: tag("name").unwrap_or_else(|| "file".into()),
        key: tag("decryption-key")?,
        nonce: tag("decryption-nonce")?,
    };
    let as_attachment = crate::group::Attachment {
        hash: f.hash.clone(),
        sha256: f.sha256.clone(),
        size: f.size.max(1),
        mime: f.mime.clone(),
        name: f.name.clone(),
    };
    as_attachment.is_valid().then_some(f)
}

/// Who a wrap is for: its one `p`. `None` if it has none, or several.
pub fn recipient_of(wrap: &Event) -> Option<PublicKey> {
    let mut ps = wrap.tags.public_keys();
    let first = ps.next()?;
    ps.next().is_none().then_some(first)
}

/// A person's inbox list: the anchors (npub, and the device to dial) that
/// keep their DMs while they are away. `["anchor", <hex>, <device>]` is
/// ours; other clients ignore it, and find no `relay` rows until a person
/// opts in to public relays (later).
pub fn inbox_list(anchors: &[(PublicKey, EndpointId)]) -> EventBuilder {
    let mut b = EventBuilder::new(INBOX_LIST, "");
    for (p, d) in anchors {
        b = b.tag(Tag::custom("anchor", [p.to_hex(), d.to_string()]));
    }
    b
}

/// The anchors an inbox list names.
pub fn inboxes_of(list: &Event) -> Vec<(PublicKey, Option<EndpointId>)> {
    if list.kind != INBOX_LIST {
        return Vec::new();
    }
    list.tags
        .iter()
        .filter_map(|t| match t.as_slice() {
            [n, p, rest @ ..] if n == "anchor" => Some((
                PublicKey::from_hex(p).ok()?,
                rest.first().and_then(|d| EndpointId::from_str(d).ok()),
            )),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_senders_binding_rides_sealed_and_only_the_senders() {
        use crate::{
            DeviceKey,
            binding::{DEFAULT_TTL, sign_binding},
        };
        let (alice, bob, eve) = (Person::generate(), Person::generate(), Person::generate());
        let now = Timestamp::now();
        let hers = sign_binding(&alice, &DeviceKey::generate(), now, DEFAULT_TTL).unwrap();
        let (to_bob, _) = alice.wrap_dm(&bob.public_key(), "hi", Some(&hers)).unwrap();
        assert!(
            !to_bob.as_json().contains(&hers.id.to_hex()),
            "visible outside the seal"
        );
        assert_eq!(bob.open_dm(&to_bob).unwrap().device, Some(hers));
        // Eve's binding in Alice's DM is not Alice's device: dropped.
        let evil = sign_binding(&eve, &DeviceKey::generate(), now, DEFAULT_TTL).unwrap();
        let (to_bob, _) = alice.wrap_dm(&bob.public_key(), "hi", Some(&evil)).unwrap();
        assert_eq!(bob.open_dm(&to_bob).unwrap().device, None);
    }

    #[test]
    fn a_dm_opens_for_its_recipient_and_its_sender_and_nobody_else() {
        let (alice, bob, eve) = (Person::generate(), Person::generate(), Person::generate());
        let (to_bob, to_alice) = alice.wrap_dm(&bob.public_key(), "hello bob", None).unwrap();
        // The outside says only: someone (not Alice), to Bob.
        assert_eq!(to_bob.kind, GIFT_WRAP);
        assert_ne!(to_bob.pubkey, alice.public_key());
        assert_eq!(recipient_of(&to_bob), Some(bob.public_key()));
        assert!(!to_bob.content.contains("hello"));
        assert!(!to_bob.as_json().contains(&alice.public_key().to_hex()));

        let at_bob = bob.open_dm(&to_bob).unwrap();
        assert_eq!(
            (at_bob.from, at_bob.to, at_bob.text.as_str()),
            (alice.public_key(), bob.public_key(), "hello bob")
        );
        let at_alice = alice.open_dm(&to_alice).unwrap();
        assert_eq!(at_alice.id, at_bob.id, "one message, two wraps");
        assert!(eve.open_dm(&to_bob).is_none());
        assert!(
            alice.open_dm(&to_bob).is_none(),
            "the recipient's wrap is not the sender's"
        );
    }

    #[test]
    fn a_dm_file_is_ciphertext_outside_and_opens_only_with_its_key() {
        let (alice, bob) = (Person::generate(), Person::generate());
        let plain = b"a holiday photo, really".repeat(100);
        let (sealed, key, nonce, ox, x) = seal_file(&plain).unwrap();
        assert!(!sealed.windows(10).any(|w| w == &plain[..10]));
        let file = DmFile {
            hash: "f".repeat(64),
            sha256: x,
            original_sha256: ox,
            size: plain.len() as u64,
            mime: "image/jpeg".into(),
            name: "holiday.jpg".into(),
            key,
            nonce,
        };
        let (to_bob, _) = alice.wrap_dm_file(&bob.public_key(), &file, None).unwrap();
        // Nothing about the file shows outside the wrap.
        let outside = to_bob.as_json();
        assert!(!outside.contains(&file.key) && !outside.contains("holiday"));
        let got = bob.open_dm(&to_bob).unwrap();
        assert_eq!(got.file.as_ref(), Some(&file));
        assert_eq!(open_file(&file, &sealed).unwrap(), plain);
        // A wrong key, or tampered bytes, open nothing.
        let wrong = DmFile {
            key: "0".repeat(64),
            ..file.clone()
        };
        assert!(open_file(&wrong, &sealed).is_none());
        let mut tampered = sealed.clone();
        tampered[5] ^= 1;
        assert!(open_file(&file, &tampered).is_none());
        // And the key never reaches a log line.
        assert!(!format!("{file:?}").contains(&file.key));
    }

    #[test]
    fn an_inbox_list_round_trips() {
        let a = Person::generate().public_key();
        let d = iroh_base::SecretKey::generate().public();
        let p = Person::generate();
        let list = p.sign(inbox_list(&[(a, d)])).unwrap();
        assert_eq!(inboxes_of(&list), vec![(a, Some(d))]);
    }
}
