//! Pure invitation rules shared by every part of the backend.
//!
//! These are byte-for-byte ports of the TypeScript in the original app
//! (`src/lib/invite-code.ts`, `src/lib/qr-payload.ts`). Printed cards and
//! already-sent e-vouchers depend on them, so `tests/golden.rs` replays
//! vectors generated from the TypeScript and every one must match.

pub mod invite_code;
pub mod qr_payload;

pub use invite_code::{
    CategoryCode, ParsedInviteCode, build_invite_code, normalize_invite_code, parse_invite_code,
    suggest_prefix, to_category_code,
};
pub use qr_payload::{PAYLOAD_VERSION, QrFields, QrRejection, build_qr_payload, verify_qr_payload};
