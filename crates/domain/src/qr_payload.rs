//! Signed QR payload: `DWI1.AJ27.FAM.0042.4.a9f2c1b8e4d7`.
//!
//! HMAC-SHA256 over the body, truncated to 12 hex characters. The secret
//! (`INVITE_SIGNING_SECRET`) never leaves the server; scanners validate codes
//! against their downloaded guest snapshot and the server verifies the
//! signature when check-ins sync.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::invite_code::{CategoryCode, build_invite_code};

pub const PAYLOAD_VERSION: &str = "DWI1";
const SIGNATURE_LEN: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrFields {
    pub prefix: String,
    pub category: CategoryCode,
    pub sequence: u32,
    /// Total people admitted.
    pub admits: u32,
}

impl QrFields {
    /// The invite code this payload refers to.
    pub fn code(&self) -> String {
        build_invite_code(&self.prefix, self.category, self.sequence)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrRejection {
    Malformed,
    UnknownVersion,
    BadSignature,
}

impl QrRejection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::UnknownVersion => "unknown_version",
            Self::BadSignature => "bad_signature",
        }
    }
}

fn body(f: &QrFields) -> String {
    format!("{PAYLOAD_VERSION}.{}.{}.{:04}.{}", f.prefix, f.category, f.sequence, f.admits)
}

fn sign(secret: &[u8], body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body.as_bytes());
    let mut hex = hex::encode(mac.finalize().into_bytes());
    hex.truncate(SIGNATURE_LEN);
    hex
}

pub fn build_qr_payload(secret: &[u8], fields: &QrFields) -> String {
    let b = body(fields);
    let sig = sign(secret, &b);
    format!("{b}.{sig}")
}

pub fn verify_qr_payload(secret: &[u8], raw: &str) -> Result<QrFields, QrRejection> {
    let parts: Vec<&str> = raw.trim().split('.').collect();
    let [version, prefix, category, seq, admits, signature] = parts.as_slice() else {
        return Err(QrRejection::Malformed);
    };
    if *version != PAYLOAD_VERSION {
        return Err(QrRejection::UnknownVersion);
    }
    let category = CategoryCode::parse(category).ok_or(QrRejection::Malformed)?;
    let digits = |s: &str, min: usize, max: usize| {
        (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(seq, 4, 4) || !digits(admits, 1, 2) {
        return Err(QrRejection::Malformed);
    }
    if signature.len() != SIGNATURE_LEN || !signature.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(QrRejection::Malformed);
    }
    let fields = QrFields {
        prefix: (*prefix).to_string(),
        category,
        sequence: seq.parse().map_err(|_| QrRejection::Malformed)?,
        admits: admits.parse().map_err(|_| QrRejection::Malformed)?,
    };
    let expected = sign(secret, &body(&fields));
    if expected.as_bytes().ct_eq(signature.as_bytes()).into() {
        Ok(fields)
    } else {
        Err(QrRejection::BadSignature)
    }
}
