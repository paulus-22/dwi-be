use clap::Args;

/// Everything comes from the environment (or `.env` in development).
#[derive(Debug, Clone, Args)]
pub struct Config {
    /// Postgres connection string.
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// Optional. Enables caching, login rate limits and multi-instance live updates.
    #[arg(long, env = "REDIS_URL")]
    pub redis_url: Option<String>,

    /// Signs admin session tokens (JWT, HS256). At least 32 bytes.
    #[arg(long, env = "JWT_SECRET", hide_env_values = true)]
    pub jwt_secret: String,

    /// Same value as the old app's INVITE_SIGNING_SECRET — QR signatures depend on it.
    #[arg(long, env = "INVITE_SIGNING_SECRET", hide_env_values = true)]
    pub invite_signing_secret: String,

    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    pub bind_addr: String,

    /// Comma-separated browser origins allowed to call the API.
    #[arg(long, env = "CORS_ORIGINS", default_value = "http://localhost:3000,http://localhost:5173")]
    pub cors_origins: String,

    /// Admin session length.
    #[arg(long, env = "SESSION_HOURS", default_value_t = 12)]
    pub session_hours: i64,

    /// Base URL of the public invitation site; WhatsApp invite links are
    /// `{PUBLIC_SITE_URL}/e/{slug}?to={token}`.
    #[arg(long, env = "PUBLIC_SITE_URL", default_value = "http://localhost:3000")]
    pub public_site_url: String,

    /// Optional. Fonnte device token for WhatsApp messages. Without it nothing
    /// is sent and each attempt is logged as FAILED.
    #[arg(long, env = "FONNTE_TOKEN", hide_env_values = true)]
    pub fonnte_token: Option<String>,

    #[command(flatten)]
    pub storage: StorageConfig,
}

/// S3-compatible object storage (Cloudflare R2 in production). All optional:
/// without it, photo uploads answer 503 and everything else works.
#[derive(Debug, Clone, Args)]
pub struct StorageConfig {
    /// e.g. https://<account>.r2.cloudflarestorage.com
    #[arg(long, env = "S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,
    #[arg(long, env = "S3_BUCKET")]
    pub s3_bucket: Option<String>,
    #[arg(long, env = "S3_REGION", default_value = "auto")]
    pub s3_region: String,
    #[arg(long, env = "S3_ACCESS_KEY_ID", hide_env_values = true)]
    pub s3_access_key_id: Option<String>,
    #[arg(long, env = "S3_SECRET_ACCESS_KEY", hide_env_values = true)]
    pub s3_secret_access_key: Option<String>,
    /// Public base URL for reading photos (an R2 custom domain). When unset,
    /// photo URLs are short-lived signed GETs.
    #[arg(long, env = "S3_PUBLIC_BASE_URL")]
    pub s3_public_base_url: Option<String>,
}

impl Config {
    pub fn cors_origins(&self) -> Vec<String> {
        self.cors_origins
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.jwt_secret.len() >= 32, "JWT_SECRET must be at least 32 bytes");
        anyhow::ensure!(!self.invite_signing_secret.is_empty(), "INVITE_SIGNING_SECRET is empty");
        Ok(())
    }
}
