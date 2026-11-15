//! Replays vectors generated from the original TypeScript implementation
//! (`tests/golden.json`). If any of these fail, printed cards would stop
//! scanning — do not "fix" the vectors, fix the port.

use dwi_domain::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    secret: String,
    codes: Vec<CodeVec>,
    payloads: Vec<PayloadVec>,
    parse: Vec<ParseVec>,
    verify: Vec<VerifyVec>,
    categories: Vec<CategoryVec>,
    prefixes: Vec<PrefixVec>,
}
#[derive(Deserialize)]
struct CodeVec {
    prefix: String,
    category: String,
    sequence: u32,
    code: String,
}
#[derive(Deserialize)]
struct PayloadVec {
    prefix: String,
    category: String,
    sequence: u32,
    admits: u32,
    payload: String,
}
#[derive(Deserialize)]
struct ParseVec {
    raw: String,
    result: Option<String>,
}
#[derive(Deserialize)]
struct VerifyVec {
    raw: String,
    valid: bool,
    reason: Option<String>,
    code: Option<String>,
}
#[derive(Deserialize)]
struct CategoryVec {
    raw: String,
    code: String,
}
#[derive(Deserialize)]
struct PrefixVec {
    slug: String,
    year: i32,
    prefix: String,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("golden.json")).unwrap()
}

fn cat(s: &str) -> CategoryCode {
    CategoryCode::parse(s).unwrap()
}

#[test]
fn invite_codes_match_typescript() {
    let g = golden();
    for v in &g.codes {
        assert_eq!(build_invite_code(&v.prefix, cat(&v.category), v.sequence), v.code);
        assert_eq!(parse_invite_code(&v.code).map(|p| p.code), Some(v.code.clone()));
    }
    assert_eq!(g.codes.len(), 175);
}

#[test]
fn parsing_matches_typescript() {
    for v in golden().parse {
        assert_eq!(parse_invite_code(&v.raw).map(|p| p.code), v.result, "raw={:?}", v.raw);
    }
}

#[test]
fn qr_payloads_match_typescript() {
    let g = golden();
    let secret = g.secret.as_bytes();
    for v in &g.payloads {
        let fields = QrFields {
            prefix: v.prefix.clone(),
            category: cat(&v.category),
            sequence: v.sequence,
            admits: v.admits,
        };
        assert_eq!(build_qr_payload(secret, &fields), v.payload);
        assert_eq!(verify_qr_payload(secret, &v.payload), Ok(fields));
    }
}

#[test]
fn verification_matches_typescript() {
    let g = golden();
    for v in &g.verify {
        let got = verify_qr_payload(g.secret.as_bytes(), &v.raw);
        assert_eq!(got.is_ok(), v.valid, "raw={}", v.raw);
        match got {
            Ok(f) => assert_eq!(Some(f.code()), v.code),
            Err(e) => assert_eq!(Some(e.as_str().to_string()), v.reason),
        }
    }
}

#[test]
fn categories_and_prefixes_match_typescript() {
    let g = golden();
    for v in g.categories {
        assert_eq!(to_category_code(Some(&v.raw)).as_str(), v.code, "raw={:?}", v.raw);
    }
    for v in g.prefixes {
        assert_eq!(suggest_prefix(&v.slug, v.year), v.prefix, "slug={}", v.slug);
    }
}

#[test]
fn wrong_secret_is_rejected() {
    let g = golden();
    let p = &g.payloads[0].payload;
    assert_eq!(verify_qr_payload(b"other", p), Err(QrRejection::BadSignature));
}
