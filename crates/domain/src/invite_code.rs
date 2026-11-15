//! Invite codes: `AJ27-FAM-0042-K`.
//!
//! ```text
//! AJ27-FAM-0042-K
//! ─┬── ─┬─ ─┬── ┬
//!  │    │   │   └ check character
//!  │    │   └ sequence within the event
//!  │    └ guest category
//!  └ event prefix
//! ```

use std::fmt;

/// No 0/O, 1/I/L, or 5/S — the pairs people confuse when reading aloud.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRTUVWXYZ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CategoryCode {
    Fam,
    Frn,
    Wrk,
    Vip,
    Gen,
}

impl CategoryCode {
    pub const ALL: [CategoryCode; 5] = [Self::Fam, Self::Frn, Self::Wrk, Self::Vip, Self::Gen];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fam => "FAM",
            Self::Frn => "FRN",
            Self::Wrk => "WRK",
            Self::Vip => "VIP",
            Self::Gen => "GEN",
        }
    }

    /// Indonesian label shown in the admin and the scanner.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fam => "keluarga",
            Self::Frn => "teman",
            Self::Wrk => "kolega",
            Self::Vip => "vip",
            Self::Gen => "umum",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

impl fmt::Display for CategoryCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maps a free-text admin category onto a code, defaulting to GEN.
pub fn to_category_code(raw: Option<&str>) -> CategoryCode {
    let Some(raw) = raw else { return CategoryCode::Gen };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return CategoryCode::Gen;
    }
    if let Some(c) = CategoryCode::parse(&trimmed.to_uppercase()) {
        return c;
    }
    match trimmed.to_lowercase().as_str() {
        "keluarga" | "famili" | "family" | "saudara" => CategoryCode::Fam,
        "teman" | "sahabat" | "friend" | "friends" => CategoryCode::Frn,
        "kolega" | "kantor" | "kerja" | "rekan" | "work" | "colleague" | "office" => CategoryCode::Wrk,
        "vip" | "vvip" | "undangan" => CategoryCode::Vip,
        _ => CategoryCode::Gen,
    }
}

/// Position-weighted checksum. The weighting catches transpositions.
fn check_character(body: &str) -> char {
    let total: usize = body
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            let weight = ALPHABET.iter().position(|&a| a == b).map_or(1, |p| p + 1);
            weight * (i + 2)
        })
        .sum();
    ALPHABET[total % ALPHABET.len()] as char
}

fn body(prefix: &str, category: CategoryCode, sequence: u32) -> String {
    format!("{prefix}-{category}-{sequence:04}")
}

pub fn build_invite_code(prefix: &str, category: CategoryCode, sequence: u32) -> String {
    let b = body(prefix, category, sequence);
    let check = check_character(&b);
    format!("{b}-{check}")
}

/// Upper-cased, whitespace stripped.
pub fn normalize_invite_code(raw: &str) -> String {
    raw.trim().to_uppercase().split_whitespace().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInviteCode {
    pub code: String,
    pub prefix: String,
    pub category: CategoryCode,
    pub sequence: u32,
}

/// Parses and verifies a code; `None` when the shape or check character is wrong.
pub fn parse_invite_code(raw: &str) -> Option<ParsedInviteCode> {
    let code = normalize_invite_code(raw);
    let mut parts = code.split('-');
    let (prefix, category, seq, check) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let ok_prefix = (2..=6).contains(&prefix.len())
        && prefix.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    let ok_cat = category.len() == 3 && category.bytes().all(|b| b.is_ascii_uppercase());
    let ok_seq = seq.len() == 4 && seq.bytes().all(|b| b.is_ascii_digit());
    let ok_check = check.len() == 1 && check.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if !(ok_prefix && ok_cat && ok_seq && ok_check) {
        return None;
    }
    let category = CategoryCode::parse(category)?;
    let sequence: u32 = seq.parse().ok()?;
    if check_character(&body(prefix, category, sequence)).to_string() != check {
        return None;
    }
    Some(ParsedInviteCode { prefix: prefix.to_string(), category, sequence, code })
}

/// "alex-ajeng" + 2027 -> "AJ27". Only a starting point; it is editable.
pub fn suggest_prefix(slug: &str, year: i32) -> String {
    let words: Vec<String> = slug
        .split('-')
        .map(|w| w.to_uppercase().chars().filter(|c| c.is_ascii_uppercase()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect();

    let first = words.first().and_then(|w| w.chars().next()).unwrap_or('X');
    let second_word: Vec<char> = words.get(1).map(|w| w.chars().collect()).unwrap_or_default();
    let second = match second_word.first() {
        Some(&c) if c != first => c,
        _ => second_word
            .get(1)
            .copied()
            .or_else(|| words.first().and_then(|w| w.chars().nth(1)))
            .unwrap_or('X'),
    };
    let yy = year.to_string();
    let yy = &yy[yy.len().saturating_sub(2)..];
    format!("{first}{second}{yy}")
}
