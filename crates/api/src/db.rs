use sqlx::PgPool;
use sqlx::migrate::Migrator;

pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Runs migrations, first adopting a database the old Prisma app created.
///
/// `0001_baseline` is the Prisma schema verbatim. On production that schema
/// already exists, so running it would fail on the first CREATE TYPE. Instead,
/// when the Prisma tables are present and sqlx has no record yet, the baseline
/// is recorded as applied (with its real checksum) and only later migrations run.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    adopt_prisma_database(pool).await?;
    MIGRATOR.run(pool).await?;
    Ok(())
}

async fn adopt_prisma_database(pool: &PgPool) -> anyhow::Result<()> {
    let has_prisma_schema: bool =
        sqlx::query_scalar("SELECT to_regclass('public.event') IS NOT NULL").fetch_one(pool).await?;
    if !has_prisma_schema {
        return Ok(());
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    let baseline = MIGRATOR.iter().find(|m| m.version == 1).expect("0001_baseline exists");
    let recorded: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 1)")
            .fetch_one(pool)
            .await?;
    if recorded {
        return Ok(());
    }

    // The newest Prisma migration adds event."giftAddress"; refuse to adopt a
    // database that is behind, rather than silently skipping its migrations.
    let up_to_date: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'event' AND column_name = 'giftAddress')",
    )
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(
        up_to_date,
        "the database has the old app's tables but not its latest migration; \
         run `bunx prisma migrate deploy` in the old app first"
    );

    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES ($1, $2, true, $3, 0)",
    )
    .bind(baseline.version)
    .bind(baseline.description.as_ref())
    .bind(baseline.checksum.as_ref())
    .execute(pool)
    .await?;
    tracing::info!("adopted existing Prisma schema as migration 0001_baseline");
    Ok(())
}
