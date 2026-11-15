use clap::{Parser, Subcommand};
use dwi_api::redis_ext::Redis;
use dwi_api::{AppState, Config, db, routes, seed};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "dwi-api", about = "DWI backend")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run migrations, then serve HTTP (the default).
    Serve(Box<Config>),
    /// Run migrations and exit.
    Migrate {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
    },
    /// Create the Alex & Ajeng demo event and a super admin (development only).
    SeedDemo {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
        #[arg(long, default_value = "admin@demo.local")]
        email: String,
        #[arg(long, env = "DEMO_ADMIN_PASSWORD")]
        password: String,
    },
}

async fn pool(url: &str) -> anyhow::Result<sqlx::PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,dwi_api=debug,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let command = match cli.command {
        Some(c) => c,
        None => Command::Serve(Box::new(ServeOnly::parse_from(["dwi-api"]).config)),
    };

    match command {
        Command::Migrate { database_url } => {
            db::migrate(&pool(&database_url).await?).await?;
            tracing::info!("migrations up to date");
        }
        Command::SeedDemo { database_url, email, password } => {
            let p = pool(&database_url).await?;
            db::migrate(&p).await?;
            let demo = seed::seed_demo(&p, &email, &password).await?;
            println!(
                "event {} ({})\nadmin {email}\ncodes:\n  {}",
                seed::DEMO_SLUG,
                demo.event_id,
                demo.codes.join("\n  ")
            );
        }
        Command::Serve(config) => serve(*config).await?,
    }
    Ok(())
}

/// `dwi-api` with no subcommand reads the serve config from the environment.
#[derive(Parser)]
struct ServeOnly {
    #[command(flatten)]
    config: Config,
}

async fn serve(config: Config) -> anyhow::Result<()> {
    config.validate()?;
    let db = pool(&config.database_url).await?;
    db::migrate(&db).await?;

    let redis = match &config.redis_url {
        Some(url) => match Redis::connect(url, "dwi").await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e, "redis unavailable; running without cache or cross-instance live updates");
                None
            }
        },
        None => None,
    };
    let state = AppState::new(&config, db, redis)?;
    state.live.spawn_redis_relay();

    let app = routes::router(state, &config.cors_origins());
    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).with_graceful_shutdown(stop_signal()).await?;
    Ok(())
}

/// Ctrl-C locally, SIGTERM from the platform on deploy.
async fn stop_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("ctrl-c handler") };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("stopping");
}
