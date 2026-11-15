//! Response shapes shared by several routes. Field names are camelCase to
//! match the scanner's TypeScript types (`apps/scanner/src/domain/types.ts`).

use chrono::{Datelike, FixedOffset, NaiveDateTime, TimeZone, Utc, Weekday};
use dwi_domain::{CategoryCode, parse_invite_code, to_category_code};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub id: Uuid,
    pub slug: String,
    pub status: String,
    pub kind: String,
    // WEDDING-only; nullable since migration 0003 (GENERIC events have none).
    pub bride_short_name: Option<String>,
    pub groom_short_name: Option<String>,
    // GENERIC-only (also settable on WEDDING events, but unused there today).
    pub title: Option<String>,
    pub subtitle: Option<String>,
    #[serde(skip)]
    pub headline_date: NaiveDateTime,
    #[serde(skip)]
    pub timezone: String,
    pub hashtag: Option<String>,
    pub code_prefix: String,
    pub venue: Option<String>,
}

/// Prisma columns are camelCase; alias them once here.
pub const EVENT_SELECT: &str = r#"
    SELECT e."id", e."slug", e."status"::text AS status, e."kind"::text AS kind,
           e."brideShortName" AS bride_short_name, e."groomShortName" AS groom_short_name,
           e."title", e."subtitle",
           e."headlineDate" AS headline_date, e."timezone", e."hashtag", e."codePrefix" AS code_prefix,
           (SELECT c."venueName" FROM "ceremony" c WHERE c."eventId" = e."id"
             ORDER BY c."sortOrder", c."startsAt" LIMIT 1) AS venue
    FROM "event" e"#;

/// What a scanner (or the admin dashboard) needs to show which event this is.
/// For `kind = "WEDDING"`, `displayName` is the couple line; for `"GENERIC"`
/// it's `title`. Both bride/groom and title/subtitle are still included
/// (possibly null) so a client can fall back to its own rendering.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventInfo {
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub display_name: String,
    pub couple_line: Option<String>,
    pub bride_short_name: Option<String>,
    pub groom_short_name: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub date_label: String,
    pub venue: String,
    pub hashtag: Option<String>,
}

impl From<&EventRow> for EventInfo {
    fn from(e: &EventRow) -> Self {
        let couple_line = (e.bride_short_name.is_some() || e.groom_short_name.is_some())
            .then(|| couple_line(e.bride_short_name.as_deref().unwrap_or(""), e.groom_short_name.as_deref().unwrap_or("")));
        let display_name = if e.kind == "GENERIC" {
            e.title.clone().unwrap_or_else(|| e.slug.clone())
        } else {
            couple_line.clone().unwrap_or_else(|| e.slug.clone())
        };
        EventInfo {
            id: e.id,
            slug: e.slug.clone(),
            kind: e.kind.clone(),
            display_name,
            couple_line,
            bride_short_name: e.bride_short_name.clone(),
            groom_short_name: e.groom_short_name.clone(),
            title: e.title.clone(),
            subtitle: e.subtitle.clone(),
            date_label: date_label_id(e.headline_date, &e.timezone),
            venue: e.venue.clone().unwrap_or_default(),
            hashtag: e.hashtag.clone(),
        }
    }
}

/// The bride leads, everywhere (same rule as the old app's `coupleLine`).
pub fn couple_line(bride: &str, groom: &str) -> String {
    format!("{bride} & {groom}")
}

/// Indonesia's three zones cover every event; anything else falls back to WIB.
fn offset_for(tz: &str) -> FixedOffset {
    let hours = match tz {
        "Asia/Makassar" | "Asia/Ujung_Pandang" => 8,
        "Asia/Jayapura" => 9,
        _ => 7,
    };
    FixedOffset::east_opt(hours * 3600).expect("valid offset")
}

/// "Sabtu, 15 Mei 2027", in the event's own timezone.
pub fn date_label_id(at_utc: NaiveDateTime, tz: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Januari",
        "Februari",
        "Maret",
        "April",
        "Mei",
        "Juni",
        "Juli",
        "Agustus",
        "September",
        "Oktober",
        "November",
        "Desember",
    ];
    let local = Utc.from_utc_datetime(&at_utc).with_timezone(&offset_for(tz));
    let day = match local.weekday() {
        Weekday::Mon => "Senin",
        Weekday::Tue => "Selasa",
        Weekday::Wed => "Rabu",
        Weekday::Thu => "Kamis",
        Weekday::Fri => "Jumat",
        Weekday::Sat => "Sabtu",
        Weekday::Sun => "Minggu",
    };
    format!("{day}, {} {} {}", local.day(), MONTHS[local.month0() as usize], local.year())
}

/// The code printed on the card decides the category; the free-text admin
/// field is only a fallback for codes that do not parse.
pub fn guest_category(code: &str, admin_category: Option<&str>) -> CategoryCode {
    parse_invite_code(code).map(|p| p.category).unwrap_or_else(|| to_category_code(admin_category))
}

/// Confirmed headcount when attending, else the invitation's cap.
pub fn guest_admits(attending: Option<bool>, rsvp_count: Option<i32>, plus_allowance: i32) -> i32 {
    match (attending, rsvp_count) {
        (Some(true), Some(n)) if n > 0 => n,
        _ => 1 + plus_allowance.max(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_label_uses_event_timezone() {
        // 2027-05-15 01:00 WIB is still the 14th in UTC.
        let utc = NaiveDateTime::parse_from_str("2027-05-14 18:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(date_label_id(utc, "Asia/Jakarta"), "Sabtu, 15 Mei 2027");
        assert_eq!(date_label_id(utc, "Asia/Jayapura"), "Sabtu, 15 Mei 2027");
    }

    #[test]
    fn admits_prefers_confirmed_headcount() {
        assert_eq!(guest_admits(Some(true), Some(3), 1), 3);
        assert_eq!(guest_admits(Some(false), Some(0), 1), 2);
        assert_eq!(guest_admits(None, None, 0), 1);
    }
}
