//! Demo data for local development and tests: the "Alex & Ajeng" event the
//! scanner demo already knows, an admin, and a guest list with real codes.

use chrono::NaiveDate;
use dwi_domain::{CategoryCode, build_invite_code};
use sqlx::PgPool;
use uuid::Uuid;

pub struct Demo {
    pub event_id: Uuid,
    pub admin_id: Uuid,
    pub codes: Vec<String>,
}

pub const DEMO_SLUG: &str = "alex-ajeng";
pub const DEMO_PREFIX: &str = "AJ27";

/// (household, category, plus-ones, rsvp: None = no answer, Some((attending, count)))
const GUESTS: &[(&str, CategoryCode, i32, Option<(bool, i32)>)] = &[
    ("Keluarga Bpk. Hartono", CategoryCode::Fam, 3, Some((true, 4))),
    ("Ibu Ratna & Keluarga", CategoryCode::Fam, 2, Some((true, 2))),
    ("Dimas Pratama", CategoryCode::Frn, 1, Some((true, 2))),
    ("Sinta Maharani", CategoryCode::Frn, 0, None),
    ("Rudi Santoso", CategoryCode::Wrk, 1, Some((false, 0))),
    ("Bpk. Wijaya (Direktur)", CategoryCode::Vip, 1, Some((true, 2))),
    ("Tim Kantor Ajeng", CategoryCode::Wrk, 5, Some((true, 6))),
    ("Om Bambang & Tante Lies", CategoryCode::Fam, 1, None),
];

/// Idempotent: re-running updates the admin password and leaves data alone.
pub async fn seed_demo(db: &PgPool, admin_email: &str, admin_password: &str) -> anyhow::Result<Demo> {
    let hash = crate::routes::auth::hash_password(admin_password)?;
    let admin_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "admin_user" ("id", "email", "name", "passwordHash", "isSuperAdmin", "updatedAt")
           VALUES (gen_random_uuid(), $1, 'Admin Demo', $2, true, now())
           ON CONFLICT ("email") DO UPDATE SET "passwordHash" = EXCLUDED."passwordHash", "updatedAt" = now()
           RETURNING "id""#,
    )
    .bind(admin_email.to_lowercase())
    .bind(hash)
    .fetch_one(db)
    .await?;

    let existing: Option<Uuid> = sqlx::query_scalar(r#"SELECT "id" FROM "event" WHERE "slug" = $1"#)
        .bind(DEMO_SLUG)
        .fetch_optional(db)
        .await?;
    let codes: Vec<String> = GUESTS
        .iter()
        .enumerate()
        .map(|(i, (_, cat, _, _))| build_invite_code(DEMO_PREFIX, *cat, i as u32 + 1))
        .collect();
    if let Some(event_id) = existing {
        return Ok(Demo { event_id, admin_id, codes });
    }

    let mut tx = db.begin().await?;
    // 15 May 2027, 10:00 WIB = 03:00 UTC.
    let headline = NaiveDate::from_ymd_opt(2027, 5, 15).unwrap().and_hms_opt(3, 0, 0).unwrap();
    let event_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO "event" ("id", "slug", "status", "brideShortName", "brideFullName", "groomShortName",
                                "groomFullName", "headlineDate", "hashtag", "codePrefix", "updatedAt")
           VALUES (gen_random_uuid(), $1, 'PUBLISHED', 'Ajeng', 'Ajeng Pramesti', 'Alex', 'Alexander Wiratama',
                   $2, '#AlexAjengForever', $3, now())
           RETURNING "id""#,
    )
    .bind(DEMO_SLUG)
    .bind(headline)
    .bind(DEMO_PREFIX)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO "ceremony" ("id", "eventId", "nameId", "nameEn", "startsAt", "venueName", "sortOrder")
           VALUES (gen_random_uuid(), $1, 'Akad Nikah', 'Holy Matrimony', $2, 'Gedung Sasana Kriya, TMII', 0)"#,
    )
    .bind(event_id)
    .bind(headline)
    .execute(&mut *tx)
    .await?;

    for (i, (name, _, plus, rsvp)) in GUESTS.iter().enumerate() {
        let guest_id: Uuid = sqlx::query_scalar(
            r#"INSERT INTO "guest" ("id", "eventId", "householdName", "token", "sequence", "plusGuestAllowance",
                                    "category", "codeLockedAt", "updatedAt")
               VALUES (gen_random_uuid(), $1, $2, $3, $4, $5, $6, now(), now()) RETURNING "id""#,
        )
        .bind(event_id)
        .bind(name)
        .bind(&codes[i])
        .bind(i as i32 + 1)
        .bind(plus)
        .bind(GUESTS[i].1.label())
        .fetch_one(&mut *tx)
        .await?;
        if let Some((attending, count)) = rsvp {
            sqlx::query(
                r#"INSERT INTO "rsvp" ("id", "guestId", "attending", "guestCount", "updatedAt")
                   VALUES (gen_random_uuid(), $1, $2, $3, now())"#,
            )
            .bind(guest_id)
            .bind(attending)
            .bind(count)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Demo { event_id, admin_id, codes })
}
