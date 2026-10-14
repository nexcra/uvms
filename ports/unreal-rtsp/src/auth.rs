use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub enum Challenge {
    Basic,
    Digest(HashMap<String, String>),
}
impl Challenge {
    pub fn parse(text: &str) -> Result<Self> {
        let (kind, args) = text.split_once(' ').unwrap_or((text, ""));
        if kind.eq_ignore_ascii_case("Basic") {
            return Ok(Self::Basic);
        }
        ensure!(
            kind.eq_ignore_ascii_case("Digest"),
            "Unsupported RTSP authentication scheme"
        );
        let bytes = args.as_bytes();
        let mut at = 0;
        let mut fields = HashMap::new();
        while at < bytes.len() {
            while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b',') {
                at += 1;
            }
            if at == bytes.len() {
                break;
            }
            let start = at;
            while at < bytes.len() && bytes[at] != b'=' {
                at += 1;
            }
            ensure!(at < bytes.len(), "Malformed digest challenge");
            let key = args[start..at].trim().to_ascii_lowercase();
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            let value = if bytes.get(at) == Some(&b'"') {
                at += 1;
                let mut value = String::new();
                let mut closed = false;
                while at < bytes.len() {
                    let c = bytes[at];
                    at += 1;
                    if c == b'"' {
                        closed = true;
                        break;
                    }
                    if c == b'\\' {
                        ensure!(at < bytes.len(), "Malformed digest escape");
                        value.push(bytes[at] as char);
                        at += 1;
                    } else {
                        value.push(c as char);
                    }
                }
                ensure!(closed, "Unterminated digest value");
                value
            } else {
                let start = at;
                while at < bytes.len() && bytes[at] != b',' {
                    at += 1;
                }
                args[start..at].trim().to_owned()
            };
            fields.insert(key, value);
        }
        ensure!(
            fields.contains_key("realm") && fields.contains_key("nonce"),
            "Incomplete digest challenge"
        );
        Ok(Self::Digest(fields))
    }
    pub fn authorization(
        &self,
        user: &str,
        password: &str,
        method: &str,
        uri: &str,
        count: u32,
    ) -> Result<String> {
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce)
            .map_err(|_| anyhow::anyhow!("Cannot generate digest nonce"))?;
        let cnonce = nonce.iter().map(|v| format!("{v:02x}")).collect::<String>();
        self.authorization_with_nonce(user, password, method, uri, count, &cnonce)
    }
    fn authorization_with_nonce(
        &self,
        user: &str,
        password: &str,
        method: &str,
        uri: &str,
        count: u32,
        cnonce: &str,
    ) -> Result<String> {
        match self {
            Self::Basic => Ok(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
            )),
            Self::Digest(fields) => {
                let realm = fields.get("realm").context("Missing realm")?;
                let nonce = fields.get("nonce").context("Missing nonce")?;
                let algorithm = fields.get("algorithm").map(String::as_str).unwrap_or("MD5");
                let lower = algorithm.to_ascii_lowercase();
                ensure!(
                    ["md5", "md5-sess", "sha-256", "sha-256-sess"].contains(&lower.as_str()),
                    "Unsupported digest algorithm"
                );
                let hash = |s: &str| {
                    if lower.starts_with("sha-256") {
                        format!("{:x}", Sha256::digest(s.as_bytes()))
                    } else {
                        format!("{:x}", md5::compute(s.as_bytes()))
                    }
                };
                let mut ha1 = hash(&format!("{user}:{realm}:{password}"));
                if lower.ends_with("-sess") {
                    ha1 = hash(&format!("{ha1}:{nonce}:{cnonce}"));
                }
                let ha2 = hash(&format!("{method}:{uri}"));
                let nc = format!("{count:08x}");
                let qop = if let Some(qops) = fields.get("qop") {
                    if qops
                        .split(',')
                        .any(|s| s.trim().eq_ignore_ascii_case("auth"))
                    {
                        true
                    } else {
                        bail!("Digest qop auth is required")
                    }
                } else {
                    false
                };
                let response = if qop {
                    hash(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"))
                } else {
                    hash(&format!("{ha1}:{nonce}:{ha2}"))
                };
                let mut out=format!("Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{response}\", algorithm={algorithm}",quote(user)?,quote(realm)?,quote(nonce)?,quote(uri)?);
                if qop {
                    out.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
                } else if lower.ends_with("-sess") {
                    out.push_str(&format!(", cnonce=\"{cnonce}\""));
                }
                if let Some(opaque) = fields.get("opaque") {
                    out.push_str(&format!(", opaque=\"{}\"", quote(opaque)?));
                }
                Ok(out)
            }
        }
    }
}
fn quote(text: &str) -> Result<String> {
    ensure!(!text.contains(['\r', '\n']), "Invalid authentication field");
    Ok(text.replace('\\', "\\\\").replace('"', "\\\""))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc_digest_vector() {
        let c=Challenge::parse("Digest realm=\"testrealm@host.com\", qop=\"auth,auth-int\", nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", opaque=\"5ccc069c403ebaf9f0171e9517f40e41\"").unwrap();
        let a = c
            .authorization_with_nonce(
                "Mufasa",
                "Circle Of Life",
                "GET",
                "/dir/index.html",
                1,
                "0a4f113b",
            )
            .unwrap();
        assert!(a.contains("6629fae49393a05397450978507c4ef1"));
    }
    #[test]
    fn malformed_and_injection_rejected() {
        assert!(Challenge::parse("Digest realm=\"unterminated").is_err());
        let c = Challenge::parse("Digest realm=\"a\",nonce=\"b\"").unwrap();
        assert!(c
            .authorization_with_nonce("a\r\nb", "x", "GET", "/", 1, "n")
            .is_err());
    }
}
