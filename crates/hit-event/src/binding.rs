//! The device binding: *"npub P speaks from device D"* (`docs/DESIGN.md` §3,
//! `docs/PROTOCOL.md`).
//!
//! An addressable event of kind [`DEVICE_BINDING`], signed by the person,
//! with `d` = the device's `EndpointId` in lowercase hex, a NIP-40
//! `expiration`, and a `proof` tag: the device key's ed25519 signature over
//! the person's public key, so a person cannot claim somebody else's device.
//! A revocation is a newer event for the same `d` with a `revoked` tag and
//! no expiry or proof — a lost device cannot sign, so revoking must not need
//! it.
//!
//! **There is no signature-only entry point.** The two ways to learn "this
//! connection is npub P" both take the connection's `EndpointId`:
//! [`verify_binding`] for one event, and [`BindingBook::authenticate`] for a
//! node, which also knows about revocations. [`BindingBook::insert`] checks
//! signatures too, but answers only "stored or not", never "who".

use std::{collections::HashMap, time::Duration};

use iroh_base::{EndpointId, SecretKey as DeviceKey, Signature as DeviceSignature};
use nostr::prelude::{Event, EventBuilder, Kind, PublicKey, Tag, Timestamp};

use crate::person::{KeyError, Person};

/// Kind number of a device binding. Addressable (30000–39999), so a newer
/// event for the same `d` replaces an older one. **Provisional** — chosen
/// clear of every kind `nostr` 0.45 knows; `DESIGN.md` §15 q4 is still open.
pub const DEVICE_BINDING: Kind = Kind::Custom(30436);

/// How long a binding we sign lasts. Short on purpose: revocation spreads
/// eventually, and expiry is the backstop (`DESIGN.md` §3).
pub const DEFAULT_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The longest lifetime a verifier accepts. A binding asking for more is
/// refused rather than trimmed: it was not signed by our code.
pub const MAX_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How far in the future `created_at` may be, for clock skew.
pub const MAX_CLOCK_SKEW: Duration = Duration::from_secs(10 * 60);

/// Domain separation for the device's proof, so the device key's signature
/// can never be replayed as a signature over anything else.
const PROOF_DOMAIN: &[u8] = b"hit-enter/device-binding/1:";

const PROOF_TAG: &str = "proof";
const REVOKED_TAG: &str = "revoked";
const ALT: &str = "HIT_ENTER device binding";

/// Why a binding was refused. Variants carry no event content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    #[error("not a device binding (wrong kind)")]
    WrongKind,
    #[error("bad event id or signature")]
    BadSignature,
    #[error("no `d` tag naming a device")]
    MissingDevice,
    #[error("the `d` tag is not a lowercase-hex EndpointId")]
    NotCanonicalDevice,
    #[error("the binding names a different device than this connection")]
    WrongDevice,
    #[error("no device proof")]
    MissingProof,
    #[error("the device proof does not verify")]
    BadProof,
    #[error("no expiration")]
    MissingExpiry,
    #[error("expired")]
    Expired,
    #[error("lifetime longer than {} days", MAX_TTL.as_secs() / 86_400)]
    TtlTooLong,
    #[error("created in the future")]
    FromTheFuture,
    #[error("this device has been revoked")]
    Revoked,
    #[error("a newer binding for this device replaces this one")]
    Superseded,
}

/// Signs a binding of `device` to `person`, valid from `now` for `ttl`.
pub fn sign_binding(
    person: &Person,
    device: &DeviceKey,
    now: Timestamp,
    ttl: Duration,
) -> Result<Event, KeyError> {
    let proof = device.sign(&proof_message(&person.public_key()));
    let builder = EventBuilder::new(DEVICE_BINDING, "")
        .custom_created_at(now)
        .tags([
            Tag::identifier(device.public().to_string()),
            Tag::expiration(now + ttl),
            Tag::custom(PROOF_TAG, [hex::encode(proof.to_bytes())]),
            Tag::custom("alt", [ALT]),
        ]);
    person.sign(builder)
}

/// Signs a revocation of `device` for `person`. Needs only the device's id:
/// the device itself may be lost.
pub fn sign_revocation(
    person: &Person,
    device: EndpointId,
    now: Timestamp,
) -> Result<Event, KeyError> {
    let builder = EventBuilder::new(DEVICE_BINDING, "")
        .custom_created_at(now)
        .tags([
            Tag::identifier(device.to_string()),
            Tag::custom(REVOKED_TAG, Vec::<String>::new()),
            Tag::custom("alt", [ALT]),
        ]);
    person.sign(builder)
}

/// Whether a binding we signed should be re-signed: less than half its
/// lifetime left at `now`. A revocation or a malformed event never is.
pub fn needs_renewal(binding: &Event, now: Timestamp) -> bool {
    match (binding.tags.expiration(), is_revoked(binding)) {
        (Some(exp), false) => {
            let left = exp.as_secs().saturating_sub(now.as_secs());
            let lifetime = exp.as_secs().saturating_sub(binding.created_at.as_secs());
            left < lifetime / 2
        }
        _ => false,
    }
}

/// Verifies `event` as a binding for the connection whose peer is `conn`,
/// now. Returns the person it authenticates.
///
/// **Alone, this cannot see revocations** — a revocation is a *different*,
/// newer event. A node authenticates connections with
/// [`BindingBook::authenticate`], which does.
pub fn verify_binding(event: &Event, conn: EndpointId) -> Result<PublicKey, BindingError> {
    verify_binding_at(event, conn, Timestamp::now())
}

/// [`verify_binding`] at a given time.
pub fn verify_binding_at(
    event: &Event,
    conn: EndpointId,
    now: Timestamp,
) -> Result<PublicKey, BindingError> {
    let parsed = Parsed::check(event, now)?;
    if parsed.device_hex != conn.to_string() {
        return Err(BindingError::WrongDevice);
    }
    if parsed.revoked {
        return Err(BindingError::Revoked);
    }
    Ok(event.pubkey)
}

/// What every peer knows about bindings: the newest event per
/// (person, device), revocations included. In memory; persisting it is
/// `hit-store`'s job (M2).
#[derive(Default)]
pub struct BindingBook {
    newest: HashMap<(PublicKey, String), Event>,
}

/// What [`BindingBook::insert`] did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    /// It is now the newest for its (person, device).
    Stored,
    /// The book already holds something that replaces it.
    Stale,
}

impl BindingBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks and stores a binding or revocation, however it arrived —
    /// gossip, sync, or the first frame of a connection.
    ///
    /// Replacement follows NIP-01 for addressable events (newer `created_at`
    /// wins; on a tie, the lower id) with one exception: **on a tie, a
    /// revocation beats a binding**, so a re-sign and a revocation issued in
    /// the same second cannot resurrect a revoked device.
    pub fn insert(&mut self, event: Event, now: Timestamp) -> Result<Inserted, BindingError> {
        let parsed = Parsed::check(&event, now)?;
        let key = (event.pubkey, parsed.device_hex);
        if let Some(current) = self.newest.get(&key)
            && !crate::replace::replaces(&event, current)
        {
            return Ok(Inserted::Stale);
        }
        self.newest.insert(key, event);
        Ok(Inserted::Stored)
    }

    /// Authenticates a connection: `presented` is the binding the peer sent as
    /// its first frame, `conn` the `EndpointId` read off the connection.
    ///
    /// The presented binding must verify for `conn` on its own, *and* the
    /// newest thing the book knows for that (person, device) must not be a
    /// revocation or an expired binding.
    pub fn authenticate(
        &mut self,
        presented: &Event,
        conn: EndpointId,
        now: Timestamp,
    ) -> Result<PublicKey, BindingError> {
        let person = verify_binding_at(presented, conn, now)?;
        self.insert(presented.clone(), now)?;
        let newest = self
            .newest
            .get(&(person, conn.to_string()))
            .ok_or(BindingError::Superseded)?;
        if is_revoked(newest) {
            return Err(BindingError::Revoked);
        }
        verify_binding_at(newest, conn, now)
    }

    /// The devices `person` can be dialled at now — unrevoked, unexpired.
    /// Discovery, not authentication: the device still has to present a
    /// binding on the connection.
    pub fn devices_of(&self, person: &PublicKey, now: Timestamp) -> Vec<EndpointId> {
        self.newest
            .iter()
            .filter(|((p, _), _)| p == person)
            .filter(|(_, ev)| !is_revoked(ev) && !expired(ev, now))
            .filter_map(|((_, d), _)| d.parse().ok())
            .collect()
    }

    /// The people `device` currently speaks for — unrevoked, unexpired.
    /// For a connection that carries no binding of its own (gossip): the
    /// device proved its id in the QUIC handshake, and each binding here
    /// carries the device's own proof, so the pair is authenticated.
    pub fn persons_of(&self, device: EndpointId, now: Timestamp) -> Vec<PublicKey> {
        let d = device.to_string();
        self.newest
            .iter()
            .filter(|((_, dev), ev)| *dev == d && !is_revoked(ev) && !expired(ev, now))
            .map(|((p, _), _)| *p)
            .collect()
    }
}

/// Whether `event` is a revocation (a binding event with a `revoked` tag).
pub fn is_revoked(event: &Event) -> bool {
    event.tags.iter().any(|t| t.kind() == REVOKED_TAG)
}

fn expired(event: &Event, now: Timestamp) -> bool {
    event.tags.expiration().is_none_or(|exp| exp <= now)
}

fn proof_message(person: &PublicKey) -> Vec<u8> {
    let mut msg = PROOF_DOMAIN.to_vec();
    msg.extend_from_slice(&person.to_bytes());
    msg
}

/// A binding or revocation whose signatures, device and lifetime check out.
/// Private: holding one says nothing about any connection.
struct Parsed {
    device_hex: String,
    revoked: bool,
}

impl Parsed {
    fn check(event: &Event, now: Timestamp) -> Result<Self, BindingError> {
        if event.kind != DEVICE_BINDING {
            return Err(BindingError::WrongKind);
        }
        event.verify().map_err(|_| BindingError::BadSignature)?;
        if event.created_at > now + MAX_CLOCK_SKEW {
            return Err(BindingError::FromTheFuture);
        }

        let device_hex = event.tags.identifier().ok_or(BindingError::MissingDevice)?;
        // Compare as canonical text, not parsed keys: iroh also parses base32,
        // and two spellings of one device would be two replaceable slots — a
        // revocation in one would leave a binding alive in the other.
        let device: EndpointId = device_hex
            .parse()
            .map_err(|_| BindingError::NotCanonicalDevice)?;
        if device.to_string() != device_hex {
            return Err(BindingError::NotCanonicalDevice);
        }

        let revoked = is_revoked(event);
        if revoked {
            return Ok(Self {
                device_hex,
                revoked,
            });
        }

        let exp = event.tags.expiration().ok_or(BindingError::MissingExpiry)?;
        if exp <= now {
            return Err(BindingError::Expired);
        }
        let lifetime = exp.as_secs().saturating_sub(event.created_at.as_secs());
        if lifetime > MAX_TTL.as_secs() {
            return Err(BindingError::TtlTooLong);
        }

        let proof_hex = event
            .tags
            .iter()
            .find(|t| t.kind() == PROOF_TAG)
            .and_then(|t| t.content())
            .ok_or(BindingError::MissingProof)?;
        let proof_bytes: [u8; 64] = hex::decode(proof_hex)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(BindingError::BadProof)?;
        device
            .verify(
                &proof_message(&event.pubkey),
                &DeviceSignature::from_bytes(&proof_bytes),
            )
            .map_err(|_| BindingError::BadProof)?;

        Ok(Self {
            device_hex,
            revoked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(86_400);

    fn now() -> Timestamp {
        Timestamp::from(1_800_000_000)
    }

    /// Signs a binding-shaped event with arbitrary tags, for malformed cases.
    fn raw(person: &Person, created: Timestamp, tags: Vec<Tag>) -> Event {
        person
            .sign(
                EventBuilder::new(DEVICE_BINDING, "")
                    .custom_created_at(created)
                    .tags(tags),
            )
            .unwrap()
    }

    fn proof_tag(person: &Person, device: &DeviceKey) -> Tag {
        let sig = device.sign(&proof_message(&person.public_key()));
        Tag::custom(PROOF_TAG, [hex::encode(sig.to_bytes())])
    }

    #[test]
    fn a_binding_verifies_for_its_own_device_only() {
        let p = Person::generate();
        let (d1, d2) = (DeviceKey::generate(), DeviceKey::generate());
        let b = sign_binding(&p, &d1, now(), DEFAULT_TTL).unwrap();
        assert_eq!(
            verify_binding_at(&b, d1.public(), now()),
            Ok(p.public_key())
        );
        assert_eq!(
            verify_binding_at(&b, d2.public(), now()),
            Err(BindingError::WrongDevice)
        );
    }

    #[test]
    fn tampering_breaks_the_signature() {
        let p = Person::generate();
        let d = DeviceKey::generate();
        let b = sign_binding(&p, &d, now(), DEFAULT_TTL).unwrap();
        let mut json: serde_json_value::Value = serde_json_value::from_event(&b);
        // Point the binding at another device without re-signing.
        let other = DeviceKey::generate().public().to_string();
        json.set_d(&other);
        let forged = json.into_event();
        assert_eq!(
            verify_binding_at(&forged, other.parse().unwrap(), now()),
            Err(BindingError::BadSignature)
        );
    }

    #[test]
    fn another_persons_device_cannot_be_claimed() {
        // Mallory signs "Mallory speaks from Alice's device", with a proof
        // that Alice's device made for Alice.
        let alice = Person::generate();
        let mallory = Person::generate();
        let device = DeviceKey::generate();
        let b = raw(
            &mallory,
            now(),
            vec![
                Tag::identifier(device.public().to_string()),
                Tag::expiration(now() + DAY),
                proof_tag(&alice, &device),
            ],
        );
        assert_eq!(
            verify_binding_at(&b, device.public(), now()),
            Err(BindingError::BadProof)
        );
        // And with no proof at all.
        let b = raw(
            &mallory,
            now(),
            vec![
                Tag::identifier(device.public().to_string()),
                Tag::expiration(now() + DAY),
            ],
        );
        assert_eq!(
            verify_binding_at(&b, device.public(), now()),
            Err(BindingError::MissingProof)
        );
    }

    #[test]
    fn a_proof_from_another_device_is_refused() {
        let p = Person::generate();
        let (d, other) = (DeviceKey::generate(), DeviceKey::generate());
        let b = raw(
            &p,
            now(),
            vec![
                Tag::identifier(d.public().to_string()),
                Tag::expiration(now() + DAY),
                proof_tag(&p, &other),
            ],
        );
        assert_eq!(
            verify_binding_at(&b, d.public(), now()),
            Err(BindingError::BadProof)
        );
    }

    #[test]
    fn the_device_must_be_spelled_in_lowercase_hex() {
        let p = Person::generate();
        let d = DeviceKey::generate();
        for spelling in [d.public().to_z32(), d.public().to_string().to_uppercase()] {
            let b = raw(
                &p,
                now(),
                vec![
                    Tag::identifier(spelling),
                    Tag::expiration(now() + DAY),
                    proof_tag(&p, &d),
                ],
            );
            assert_eq!(
                verify_binding_at(&b, d.public(), now()),
                Err(BindingError::NotCanonicalDevice)
            );
        }
        let b = raw(&p, now(), vec![Tag::expiration(now() + DAY)]);
        assert_eq!(
            verify_binding_at(&b, d.public(), now()),
            Err(BindingError::MissingDevice)
        );
    }

    #[test]
    fn lifetime_rules() {
        let p = Person::generate();
        let d = DeviceKey::generate();
        let b = sign_binding(&p, &d, now(), DEFAULT_TTL).unwrap();
        assert_eq!(
            verify_binding_at(&b, d.public(), now() + DEFAULT_TTL),
            Err(BindingError::Expired)
        );

        let long = sign_binding(&p, &d, now(), MAX_TTL + DAY).unwrap();
        assert_eq!(
            verify_binding_at(&long, d.public(), now()),
            Err(BindingError::TtlTooLong)
        );

        let future = sign_binding(&p, &d, now() + DAY, DEFAULT_TTL).unwrap();
        assert_eq!(
            verify_binding_at(&future, d.public(), now()),
            Err(BindingError::FromTheFuture)
        );

        let no_exp = raw(
            &p,
            now(),
            vec![Tag::identifier(d.public().to_string()), proof_tag(&p, &d)],
        );
        assert_eq!(
            verify_binding_at(&no_exp, d.public(), now()),
            Err(BindingError::MissingExpiry)
        );
    }

    #[test]
    fn not_a_binding_kind() {
        let p = Person::generate();
        let note = p.sign(EventBuilder::new(Kind::TextNote, "hi")).unwrap();
        assert_eq!(
            verify_binding_at(&note, DeviceKey::generate().public(), now()),
            Err(BindingError::WrongKind)
        );
    }

    #[test]
    fn renewal_after_half_the_lifetime() {
        let p = Person::generate();
        let d = DeviceKey::generate();
        let b = sign_binding(&p, &d, now(), DEFAULT_TTL).unwrap();
        assert!(!needs_renewal(&b, now() + DAY));
        assert!(needs_renewal(&b, now() + Duration::from_secs(4 * 86_400)));
        let r = sign_revocation(&p, d.public(), now()).unwrap();
        assert!(!needs_renewal(&r, now() + DEFAULT_TTL));
    }

    #[test]
    fn revocation_wins_a_tie_and_a_newer_binding_wins_after() {
        let p = Person::generate();
        let d = DeviceKey::generate();
        let mut book = BindingBook::new();
        let b = sign_binding(&p, &d, now(), DEFAULT_TTL).unwrap();
        let r = sign_revocation(&p, d.public(), now()).unwrap();

        // Same second, either arrival order: revoked.
        book.insert(r.clone(), now()).unwrap();
        assert_eq!(book.insert(b.clone(), now()), Ok(Inserted::Stale));
        assert_eq!(
            book.authenticate(&b, d.public(), now()),
            Err(BindingError::Revoked)
        );
        let mut book = BindingBook::new();
        book.insert(b.clone(), now()).unwrap();
        assert_eq!(book.insert(r, now()), Ok(Inserted::Stored));
        assert_eq!(
            book.authenticate(&b, d.public(), now()),
            Err(BindingError::Revoked)
        );

        // Re-binding later is a deliberate act by the person, and it works.
        let later = now() + DAY;
        let again = sign_binding(&p, &d, later, DEFAULT_TTL).unwrap();
        assert_eq!(
            book.authenticate(&again, d.public(), later),
            Ok(p.public_key())
        );
    }

    #[test]
    fn devices_of_lists_only_live_devices() {
        let p = Person::generate();
        let (d1, d2) = (DeviceKey::generate(), DeviceKey::generate());
        let mut book = BindingBook::new();
        book.insert(sign_binding(&p, &d1, now(), DEFAULT_TTL).unwrap(), now())
            .unwrap();
        book.insert(sign_binding(&p, &d2, now(), DEFAULT_TTL).unwrap(), now())
            .unwrap();
        book.insert(
            sign_revocation(&p, d2.public(), now() + DAY).unwrap(),
            now() + DAY,
        )
        .unwrap();
        assert_eq!(
            book.devices_of(&p.public_key(), now() + DAY),
            vec![d1.public()]
        );
        assert!(
            book.devices_of(&p.public_key(), now() + DEFAULT_TTL)
                .is_empty()
        );
    }

    /// Just enough JSON surgery to forge an event without re-signing it.
    mod serde_json_value {
        use nostr::prelude::Event;

        pub struct Value(String);

        pub fn from_event(e: &Event) -> Value {
            Value(serde_json::to_string(e).unwrap())
        }

        impl Value {
            pub fn set_d(&mut self, new: &str) {
                let start = self.0.find("[\"d\",\"").unwrap() + 6;
                let end = start + self.0[start..].find('"').unwrap();
                self.0.replace_range(start..end, new);
            }
            pub fn into_event(self) -> Event {
                serde_json::from_str(&self.0).unwrap()
            }
        }
    }
}
