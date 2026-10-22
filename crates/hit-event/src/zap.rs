//! Zaps (M10, `docs/DESIGN.md` §11): NIP-57's zap request and receipt, the
//! bolt11 fields a receipt is checked against, and our **carrier** — how a
//! receipt enters a group.
//!
//! A receipt (kind 9735) is signed by the recipient's wallet provider, who is
//! not a member and carries no `h` tag, so it cannot enter a group as it is.
//! A member who fetched it wraps it whole in a [`ZAP_CARRIER`] with the
//! group's `h` and the channel of the zapped message; every peer then checks
//! the receipt inside by NIP-57's rules ([`carried`]) — the carrier is only
//! the envelope. Whose provider may sign a person's receipts is their own
//! profile's word ([`zap_pubkey_of`]), so a member who forges a receipt (a
//! valid signature from the wrong key) gets it stored and shown nowhere.
//!
//! No I/O: fetching a Lightning address, paying, and talking to relays are
//! `hit-zap`'s.

use nostr::prelude::{
    Event, EventBuilder, EventId, Kind, Metadata, PublicKey, Tag,
};

/// NIP-57's zap request: signed by the sender (or a throwaway key, `anon`),
/// handed to the recipient's LNURL server, never published by us.
pub const ZAP_REQUEST: Kind = Kind::ZapRequest;
/// NIP-57's zap receipt: signed by the recipient's wallet provider.
pub const ZAP_RECEIPT: Kind = Kind::ZapReceipt;
/// **Ours, provisional**: a zap receipt carried into a group — `["h", G]`,
/// `["channel", C]`, `["e", <zapped message>]`, `["p", <recipient>]`,
/// `["k", "9735"]`, and the receipt's JSON as the content.
pub const ZAP_CARRIER: Kind = Kind::from_u16(9436);

/// The largest receipt carried, in bytes of JSON. A real one is 1–3 kB: a
/// bolt11 and the zap request it quotes, with its relays and comment.
pub const MAX_RECEIPT_BYTES: usize = 16 * 1024;
/// The longest zap comment we send. LNURL servers cap their own lower;
/// this is only so a request stays well inside [`MAX_RECEIPT_BYTES`].
pub const MAX_COMMENT: usize = 280;

/// Our profile field for the key a person's wallet provider signs their
/// receipts with — the LNURL server's `nostrPubkey`. NIP-57 has the sender
/// fetch it from the LNURL server; a third member cannot (no HTTP in the
/// chat plane), so the recipient's app writes it here, and it is the
/// recipient's word, signed, that every peer checks a receipt against.
pub const ZAP_PUBKEY_FIELD: &str = "zap_pubkey";

/// A zap, as a validated receipt says it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zap {
    /// The receipt's id: one zap however many members carry it.
    pub receipt: EventId,
    /// The zap request's id — how a sender knows the receipt is theirs.
    pub request: EventId,
    /// Who signed the receipt — the recipient's wallet provider, if it is
    /// the key their profile names.
    pub provider: PublicKey,
    pub recipient: PublicKey,
    /// The zapped event.
    pub target: EventId,
    /// The zap request's signer — `None` for an anonymous zap, whose
    /// signer is a throwaway key.
    pub sender: Option<PublicKey>,
    pub amount_msat: u64,
    pub comment: String,
    /// When the provider says it was paid (the receipt's `created_at`).
    pub paid_at: u64,
}

/// Why a receipt is not a zap.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZapError {
    #[error("not a zap receipt")]
    NotAReceipt,
    #[error("bad signature")]
    BadSignature,
    #[error("the receipt's zap request is missing or invalid")]
    BadRequest,
    #[error("the receipt's invoice is missing or unreadable")]
    BadInvoice,
    #[error("the invoice is not for the amount requested")]
    WrongAmount,
    #[error("the receipt and its request disagree on recipient or target")]
    Mismatch,
    #[error("too large")]
    TooLarge,
}

/// Builds a NIP-57 zap request for `amount_msat` to `recipient` for their
/// event `target`, asking for the receipt on `relays`. `lnurl` is the
/// recipient's LNURL-pay URL, bech32-encoded ([`lnurl_encode`]). `anon`
/// adds NIP-57's `anon` tag; the caller then signs with a throwaway key.
pub fn request(
    recipient: &PublicKey,
    target: &EventId,
    amount_msat: u64,
    relays: &[String],
    lnurl: &str,
    comment: &str,
    anon: bool,
) -> EventBuilder {
    let comment: String = comment.chars().take(MAX_COMMENT).collect();
    let mut b = EventBuilder::new(ZAP_REQUEST, comment)
        .tag(Tag::custom("relays", relays.iter().cloned()))
        .tag(Tag::custom("amount", [amount_msat.to_string()]))
        .tag(Tag::custom("lnurl", [lnurl]))
        .tag(Tag::public_key(*recipient))
        .tag(Tag::event(*target));
    if anon {
        b = b.tag(Tag::custom("anon", Vec::<String>::new()));
    }
    b
}

/// The carrier for `receipt`, a zap of a message in `channel` of `group`.
/// The caller checks the receipt first ([`check_receipt`]); every peer
/// checks it again.
pub fn carrier(group: &PublicKey, channel: &str, receipt: &Event, zap: &Zap) -> EventBuilder {
    EventBuilder::new(ZAP_CARRIER, receipt.as_json())
        .tag(Tag::custom("h", [group.to_hex()]))
        .tag(Tag::custom("channel", [channel]))
        .tag(Tag::event(zap.target))
        .tag(Tag::public_key(zap.recipient))
        .tag(Tag::custom("k", [ZAP_RECEIPT.as_u16().to_string()]))
}

/// The zap a carrier holds, if the receipt inside passes NIP-57's checks
/// and agrees with the carrier's own `e` and `p`. Says nothing about
/// whether the receipt's signer is the recipient's provider — that is the
/// recipient's profile's to say ([`zap_pubkey_of`]), checked where totals
/// are counted.
pub fn carried(event: &Event) -> Result<Zap, ZapError> {
    if event.kind != ZAP_CARRIER {
        return Err(ZapError::NotAReceipt);
    }
    if event.content.len() > MAX_RECEIPT_BYTES {
        return Err(ZapError::TooLarge);
    }
    let receipt = Event::from_json(&event.content).map_err(|_| ZapError::NotAReceipt)?;
    let zap = check_receipt(&receipt)?;
    let e = single(event, "e").and_then(|v| EventId::from_hex(v).ok());
    let p = single(event, "p").and_then(|v| PublicKey::from_hex(v).ok());
    if e != Some(zap.target) || p != Some(zap.recipient) {
        return Err(ZapError::Mismatch);
    }
    Ok(zap)
}

/// NIP-57 Appendix F, as far as it goes without HTTP: the receipt's
/// signature, its zap request's signature and kind, the invoice's amount
/// against the request's, and the same recipient and target on both. Not
/// the invoice's description hash: NIP-57 asks it, but Primal's invoices
/// do not commit to the request, and a receipt is the provider's word
/// either way. The caller checks
/// the signer against the recipient's provider key.
pub fn check_receipt(receipt: &Event) -> Result<Zap, ZapError> {
    if receipt.kind != ZAP_RECEIPT {
        return Err(ZapError::NotAReceipt);
    }
    receipt.verify().map_err(|_| ZapError::BadSignature)?;
    let description = single(receipt, "description").ok_or(ZapError::BadRequest)?;
    let request = Event::from_json(description).map_err(|_| ZapError::BadRequest)?;
    if request.kind != ZAP_REQUEST || request.verify().is_err() {
        return Err(ZapError::BadRequest);
    }
    let invoice = single(receipt, "bolt11")
        .and_then(|b| Invoice::decode(b).ok())
        .ok_or(ZapError::BadInvoice)?;
    let asked: u64 = single(&request, "amount")
        .and_then(|a| a.parse().ok())
        .ok_or(ZapError::BadRequest)?;
    if invoice.amount_msat != Some(asked) {
        return Err(ZapError::WrongAmount);
    }
    let recipient = single(&request, "p")
        .and_then(|v| PublicKey::from_hex(v).ok())
        .ok_or(ZapError::BadRequest)?;
    let target = single(&request, "e")
        .and_then(|v| EventId::from_hex(v).ok())
        .ok_or(ZapError::BadRequest)?;
    let r_p = single(receipt, "p").and_then(|v| PublicKey::from_hex(v).ok());
    let r_e = single(receipt, "e").and_then(|v| EventId::from_hex(v).ok());
    if r_p != Some(recipient) || r_e != Some(target) {
        return Err(ZapError::Mismatch);
    }
    let anon = request.tags.iter().any(|t| t.as_slice().first().is_some_and(|n| n == "anon"));
    Ok(Zap {
        receipt: receipt.id,
        request: request.id,
        provider: receipt.pubkey,
        recipient,
        target,
        sender: (!anon).then_some(request.pubkey),
        amount_msat: asked,
        comment: request.content.chars().take(MAX_COMMENT).collect(),
        paid_at: receipt.created_at.as_secs(),
    })
}

/// The key a person's profile says their wallet provider signs receipts
/// with ([`ZAP_PUBKEY_FIELD`]).
pub fn zap_pubkey_of(profile: &Event) -> Option<PublicKey> {
    if profile.kind != Kind::Metadata {
        return None;
    }
    let meta = Metadata::try_from(profile).ok()?;
    PublicKey::from_hex(meta.custom.get(ZAP_PUBKEY_FIELD)?.as_str()?).ok()
}

/// The Lightning address a profile names (`lud16`), if it looks like one.
pub fn lud16_of(profile: &Event) -> Option<String> {
    if profile.kind != Kind::Metadata {
        return None;
    }
    let meta = Metadata::try_from(profile).ok()?;
    meta.lud16.filter(|a| a.contains('@'))
}

/// `previous` with its Lightning address and provider key set — or both
/// removed, with `None`. Everything else in it is kept.
pub fn with_lud16(previous: Option<&Event>, address: Option<(&str, &PublicKey)>) -> EventBuilder {
    let mut meta = previous
        .filter(|e| e.kind == Kind::Metadata)
        .and_then(|e| Metadata::try_from(e).ok())
        .unwrap_or_default();
    match address {
        Some((a, key)) => {
            meta.lud16 = Some(a.trim().to_string());
            meta.custom
                .insert(ZAP_PUBKEY_FIELD.to_string(), key.to_hex().into());
        }
        None => {
            meta.lud16 = None;
            meta.custom.remove(ZAP_PUBKEY_FIELD);
        }
    }
    use nostr::prelude::IntoEventBuilder;
    meta.into_event_builder()
}

fn single<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|t| match t.as_slice() {
        [n, v, ..] if n == name => Some(v.as_str()),
        _ => None,
    })
}

// --- bolt11 ---------------------------------------------------------------

/// The fields of a BOLT 11 invoice a zap is checked against. **The node's
/// signature is not checked**: NIP-57 does not ask it, and a receipt is the
/// provider's word either way (`DESIGN.md` §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    /// `bc`, `tb`, `tbs`, `bcrt`.
    pub currency: String,
    /// `None` for an invoice that lets the payer choose — never a zap's.
    pub amount_msat: Option<u64>,
    pub timestamp: u64,
    pub payment_hash: Option<[u8; 32]>,
    pub description_hash: Option<[u8; 32]>,
    pub description: Option<String>,
    /// Seconds after `timestamp`; BOLT 11's default is 3600.
    pub expiry: u64,
}

/// Why a string is not an invoice we can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a readable bolt11 invoice")]
pub struct BadInvoice;

/// Signature (512 bits) and recovery id (8 bits), in 5-bit words.
const SIGNATURE_WORDS: usize = 104;

impl Invoice {
    pub fn decode(s: &str) -> Result<Self, BadInvoice> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        let lower = lower.strip_prefix("lightning:").unwrap_or(&lower);
        let (hrp, words) = bech32::decode(lower).ok_or(BadInvoice)?;
        let rest = hrp.strip_prefix("ln").ok_or(BadInvoice)?;
        let split = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let (currency, amount) = rest.split_at(split);
        if currency.is_empty() {
            return Err(BadInvoice);
        }
        let amount_msat = if amount.is_empty() {
            None
        } else {
            Some(parse_amount(amount)?)
        };
        if words.len() < 7 + SIGNATURE_WORDS {
            return Err(BadInvoice);
        }
        let data = &words[..words.len() - SIGNATURE_WORDS];
        let timestamp = data[..7].iter().fold(0u64, |a, w| (a << 5) | u64::from(*w));
        let mut inv = Invoice {
            currency: currency.to_string(),
            amount_msat,
            timestamp,
            payment_hash: None,
            description_hash: None,
            description: None,
            expiry: 3600,
        };
        let mut i = 7;
        while i < data.len() {
            if i + 3 > data.len() {
                return Err(BadInvoice);
            }
            let kind = data[i];
            let len = (usize::from(data[i + 1]) << 5) | usize::from(data[i + 2]);
            let field = data.get(i + 3..i + 3 + len).ok_or(BadInvoice)?;
            i += 3 + len;
            match kind {
                // p: payment hash; h: description hash — 52 words, 256 bits.
                1 if len == 52 => inv.payment_hash = to_bytes(field).try_into().ok(),
                23 if len == 52 => inv.description_hash = to_bytes(field).try_into().ok(),
                13 => inv.description = String::from_utf8(to_bytes(field)).ok(),
                6 => inv.expiry = field.iter().fold(0u64, |a, w| (a << 5) | u64::from(*w)),
                _ => {}
            }
        }
        Ok(inv)
    }

    /// Encodes the fields this type holds, with a zero signature: an
    /// invoice for **tests and fakes only** — no Lightning node would pay it.
    pub fn encode_unsigned(&self) -> String {
        let mut hrp = format!("ln{}", self.currency);
        if let Some(msat) = self.amount_msat {
            hrp.push_str(&format_amount(msat));
        }
        let mut words: Vec<u8> = (0..7).rev().map(|k| ((self.timestamp >> (5 * k)) & 31) as u8).collect();
        let mut field = |kind: u8, data: Vec<u8>| {
            words.push(kind);
            words.push((data.len() >> 5) as u8);
            words.push((data.len() & 31) as u8);
            words.extend(data);
        };
        if let Some(h) = self.payment_hash {
            field(1, to_words(&h));
        }
        if let Some(h) = self.description_hash {
            field(23, to_words(&h));
        }
        if let Some(d) = &self.description {
            field(13, to_words(d.as_bytes()));
        }
        if self.expiry != 3600 {
            let mut e = Vec::new();
            let mut x = self.expiry;
            while x > 0 {
                e.insert(0, (x & 31) as u8);
                x >>= 5;
            }
            field(6, e);
        }
        words.extend([0u8; SIGNATURE_WORDS]);
        bech32::encode(&hrp, &words)
    }
}

/// BOLT 11's amount: digits and an optional multiplier, in bitcoin.
fn parse_amount(a: &str) -> Result<u64, BadInvoice> {
    let (digits, mult) = match a.chars().last() {
        Some(c @ ('m' | 'u' | 'n' | 'p')) => (&a[..a.len() - 1], Some(c)),
        _ => (a, None),
    };
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BadInvoice);
    }
    let n: u64 = digits.parse().map_err(|_| BadInvoice)?;
    let msat = match mult {
        None => n.checked_mul(100_000_000_000),
        Some('m') => n.checked_mul(100_000_000),
        Some('u') => n.checked_mul(100_000),
        Some('n') => n.checked_mul(100),
        // A pico-bitcoin is a tenth of a millisat; BOLT 11 requires a
        // multiple of ten.
        _ => n.is_multiple_of(10).then_some(n / 10),
    };
    msat.ok_or(BadInvoice)
}

fn format_amount(msat: u64) -> String {
    for (mult, per) in [("m", 100_000_000), ("u", 100_000), ("n", 100)] {
        if msat.is_multiple_of(per) {
            return format!("{}{mult}", msat / per);
        }
    }
    format!("{}p", msat * 10)
}

fn to_bytes(words: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for w in words {
        acc = (acc << 5) | u32::from(*w);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn to_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in bytes {
        acc = (acc << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 31) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 31) as u8);
    }
    out
}

/// LUD-01: an LNURL is its URL, bech32-encoded with the `lnurl` prefix —
/// what NIP-57's `lnurl` tag holds.
pub fn lnurl_encode(url: &str) -> String {
    bech32::encode("lnurl", &to_words(url.as_bytes())).to_uppercase()
}

/// The URL inside an LNURL.
pub fn lnurl_decode(lnurl: &str) -> Option<String> {
    let (hrp, words) = bech32::decode(&lnurl.trim().to_ascii_lowercase())?;
    (hrp == "lnurl").then(|| String::from_utf8(to_bytes(&words)).ok())?
}

/// Bech32 (BIP-173) without its 90-character limit, which bolt11 and LNURL
/// both exceed. Five-bit words in and out; the checksum is checked.
mod bech32 {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];

    fn polymod(values: impl Iterator<Item = u8>) -> u32 {
        let mut chk = 1u32;
        for v in values {
            let b = chk >> 25;
            chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(v);
            for (i, g) in GEN.iter().enumerate() {
                if (b >> i) & 1 == 1 {
                    chk ^= g;
                }
            }
        }
        chk
    }

    fn hrp_expand(hrp: &str) -> impl Iterator<Item = u8> + '_ {
        hrp.bytes()
            .map(|b| b >> 5)
            .chain(std::iter::once(0))
            .chain(hrp.bytes().map(|b| b & 31))
    }

    /// `(hrp, words)` of a lower-case string with a valid checksum.
    pub fn decode(s: &str) -> Option<(String, Vec<u8>)> {
        let pos = s.rfind('1')?;
        let (hrp, data) = (&s[..pos], &s[pos + 1..]);
        if hrp.is_empty() || data.len() < 6 || !hrp.bytes().all(|b| (33..=126).contains(&b)) {
            return None;
        }
        let words: Vec<u8> = data
            .bytes()
            .map(|c| CHARSET.iter().position(|x| *x == c).map(|p| p as u8))
            .collect::<Option<_>>()?;
        if polymod(hrp_expand(hrp).chain(words.iter().copied())) != 1 {
            return None;
        }
        Some((hrp.to_string(), words[..words.len() - 6].to_vec()))
    }

    pub fn encode(hrp: &str, words: &[u8]) -> String {
        let chk = polymod(
            hrp_expand(hrp)
                .chain(words.iter().copied())
                .chain([0u8; 6]),
        ) ^ 1;
        let mut s = format!("{hrp}1");
        for w in words.iter().copied().chain((0..6).map(|i| ((chk >> (5 * (5 - i))) & 31) as u8)) {
            s.push(CHARSET[usize::from(w)] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Person;
    use sha2::{Digest, Sha256};
    use nostr::prelude::{FinalizeEvent, Keys, Timestamp};

    /// BOLT 11's own test vector: "Please send $3 for a cup of coffee to
    /// the same peer, within one minute".
    const COFFEE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

    #[test]
    fn bolt11s_own_example_reads() {
        let inv = Invoice::decode(COFFEE).unwrap();
        assert_eq!(inv.currency, "bc");
        assert_eq!(inv.amount_msat, Some(250_000_000));
        assert_eq!(inv.timestamp, 1_496_314_658);
        assert_eq!(inv.description.as_deref(), Some("1 cup coffee"));
        assert_eq!(inv.expiry, 60);
        assert_eq!(
            hex::encode(inv.payment_hash.unwrap()),
            "0001020304050607080900010203040506070809000102030405060708090102"
        );
        // One character changed breaks the checksum.
        assert!(Invoice::decode(&COFFEE.replace("pvjluez", "pvjluey")).is_err());
    }

    #[test]
    fn amounts_round_trip_through_every_multiplier() {
        for msat in [1_000, 21_000, 1_000_000, 1_010_000, 123_456, 7, 100_000_000_000] {
            let inv = Invoice {
                currency: "bc".into(),
                amount_msat: Some(msat),
                timestamp: 1_700_000_000,
                payment_hash: Some([7; 32]),
                description_hash: Some([9; 32]),
                description: None,
                expiry: 600,
            };
            assert_eq!(Invoice::decode(&inv.encode_unsigned()).unwrap(), inv);
        }
    }

    #[test]
    fn an_lnurl_round_trips() {
        let url = "https://example.com/.well-known/lnurlp/alice";
        let l = lnurl_encode(url);
        assert!(l.starts_with("LNURL1"));
        assert_eq!(lnurl_decode(&l).as_deref(), Some(url));
    }

    /// A receipt as an honest provider would sign it for `request`.
    fn receipt(provider: &Keys, request: &Event, msat: u64) -> Event {
        let description = request.as_json();
        let inv = Invoice {
            currency: "bc".into(),
            amount_msat: Some(msat),
            timestamp: 1_700_000_000,
            payment_hash: Some([1; 32]),
            description_hash: Some(Sha256::digest(description.as_bytes()).into()),
            description: None,
            expiry: 3600,
        };
        nostr::nips::nip57::ZapReceipt::new(inv.encode_unsigned(), request)
            .into_event_builder()
            .custom_created_at(Timestamp::from(1_700_000_100))
            .finalize(provider)
            .unwrap()
    }
    use nostr::prelude::IntoEventBuilder;

    #[test]
    fn an_honest_receipt_checks_and_a_tampered_one_does_not() {
        let (alice, bob) = (Person::generate(), Person::generate());
        let provider = Keys::generate();
        let target = EventId::from_byte_array([0; 32]);
        let req = alice
            .sign(request(&bob.public_key(), &target, 21_000, &["wss://anchor.example".into()], "LNURL1X", "nice", false))
            .unwrap();
        let r = receipt(&provider, &req, 21_000);
        let zap = check_receipt(&r).unwrap();
        assert_eq!(zap.recipient, bob.public_key());
        assert_eq!(zap.sender, Some(alice.public_key()));
        assert_eq!(zap.provider, provider.public_key());
        assert_eq!((zap.amount_msat, zap.comment.as_str()), (21_000, "nice"));

        // Invoice for less than was asked.
        assert_eq!(check_receipt(&receipt(&provider, &req, 1_000)), Err(ZapError::WrongAmount));

        // The description swapped for another request: its hash no longer
        // matches the invoice.
        let other = alice
            .sign(request(&bob.public_key(), &target, 21_000, &[], "LNURL1X", "other", false))
            .unwrap();
        let honest = receipt(&provider, &req, 21_000);
        let mut tags: Vec<Tag> = honest.tags.iter().cloned().collect();
        for t in &mut tags {
            if t.as_slice().first().map(String::as_str) == Some("description") {
                *t = Tag::custom("description", [other.as_json()]);
            }
        }
        let swapped = EventBuilder::new(ZAP_RECEIPT, "")
            .tags(tags)
            .finalize(&provider)
            .unwrap();
        // The invoice no longer commits to the request it carries, as
        // Primal's never do: accepted, as a zap of the request it carries.
        assert_eq!(check_receipt(&swapped).map(|z| z.request), Ok(other.id));
    }

    #[test]
    fn an_anonymous_zap_names_no_sender() {
        let bob = Person::generate();
        let throwaway = Keys::generate();
        let req = request(&bob.public_key(), &EventId::from_byte_array([0; 32]), 5_000, &[], "LNURL1X", "", true)
            .finalize(&throwaway)
            .unwrap();
        let zap = check_receipt(&receipt(&Keys::generate(), &req, 5_000)).unwrap();
        assert_eq!(zap.sender, None);
    }

    #[test]
    fn a_carrier_must_agree_with_the_receipt_inside() {
        let (alice, bob, carol) = (Person::generate(), Person::generate(), Person::generate());
        let g = Keys::generate().public_key();
        let target = EventId::from_byte_array([0; 32]);
        let req = alice
            .sign(request(&bob.public_key(), &target, 21_000, &[], "LNURL1X", "", false))
            .unwrap();
        let r = receipt(&Keys::generate(), &req, 21_000);
        let zap = check_receipt(&r).unwrap();
        let c = carol.sign(carrier(&g, "general", &r, &zap)).unwrap();
        assert_eq!(carried(&c).unwrap(), zap);
        // The same receipt, carried as if it zapped someone else.
        let lie = Zap { recipient: carol.public_key(), ..zap };
        let c = carol.sign(carrier(&g, "general", &r, &lie)).unwrap();
        assert_eq!(carried(&c), Err(ZapError::Mismatch));
    }

    #[test]
    fn a_profile_names_its_provider_and_keeps_the_rest() {
        let p = Person::generate();
        let key = Keys::generate().public_key();
        let named = p.sign(crate::profile::with_name(None, "Bob")).unwrap();
        let with = p.sign(with_lud16(Some(&named), Some(("bob@coinos.io", &key)))).unwrap();
        assert_eq!(lud16_of(&with).as_deref(), Some("bob@coinos.io"));
        assert_eq!(zap_pubkey_of(&with), Some(key));
        assert_eq!(crate::profile::name_of(&with).as_deref(), Some("Bob"));
        let without = p.sign(with_lud16(Some(&with), None)).unwrap();
        assert_eq!((lud16_of(&without), zap_pubkey_of(&without)), (None, None));
    }
}
