//! A Lightning address (LUD-16): `name@domain`, which is
//! `https://domain/.well-known/lnurlp/name`.

use std::{fmt, str::FromStr};

use crate::{Error, is_loopback};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LightningAddress {
    name: String,
    domain: String,
}

impl LightningAddress {
    /// The LNURL-pay URL behind the address. `https://`, except for a
    /// loopback domain (a test's fake server), which is `http://`.
    pub fn url(&self) -> String {
        let scheme = if is_loopback(&self.domain) { "http" } else { "https" };
        format!("{scheme}://{}/.well-known/lnurlp/{}", self.domain, self.name)
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }
}

impl FromStr for LightningAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bad = || Error::Address(s.chars().take(100).collect());
        let s = s.trim().to_ascii_lowercase();
        let (name, domain) = s.rsplit_once('@').ok_or_else(bad)?;
        // LUD-16: the name is a-z0-9-_.+ ; the domain a hostname, with a
        // port only for loopback.
        let name_ok = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b));
        let host = domain.split(':').next().unwrap_or("");
        let domain_ok = !host.is_empty()
            && domain.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
            && (host.contains('.') || host == "localhost")
            && (!domain.contains(':') || is_loopback(domain));
        if !name_ok || !domain_ok {
            return Err(bad());
        }
        Ok(Self {
            name: name.to_string(),
            domain: domain.to_string(),
        })
    }
}

impl fmt::Display for LightningAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_and_name_their_url() {
        let a: LightningAddress = " Bob@Coinos.io ".parse().unwrap();
        assert_eq!(a.to_string(), "bob@coinos.io");
        assert_eq!(a.url(), "https://coinos.io/.well-known/lnurlp/bob");
        let local: LightningAddress = "bob@127.0.0.1:4000".parse().unwrap();
        assert_eq!(local.url(), "http://127.0.0.1:4000/.well-known/lnurlp/bob");
        for bad in [
            "bob",
            "@coinos.io",
            "bob@",
            "bob@coinos",
            "b ob@coinos.io",
            "bob@coinos.io:8080",
            "bob@coinos.io/x",
            "bob@evil.com@",
        ] {
            assert!(bad.parse::<LightningAddress>().is_err(), "{bad}");
        }
    }
}
