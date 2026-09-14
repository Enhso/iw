//! Binary entry point: bootstraps tracing, configuration, the mnestic
//! graph store, and the Axum service.

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use iw_server::app::{router, AppState};
use iw_server::config::{Config, DbEngine};
use iw_server::research::ResearchWorker;
use iw_server::store::GraphStore;

/// Bootstraps and runs the Axum service until the listener is closed or an
/// unrecoverable error occurs.
///
/// # Errors
/// Returns an error if configuration cannot be loaded, the graph store
/// cannot be opened or initialized, the bind address cannot be listened
/// on, or the server itself fails.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env().context("loading configuration from environment")?;

    let store = match config.db_engine {
        DbEngine::Memory => GraphStore::open_memory().context("opening in-memory graph store")?,
        DbEngine::Sqlite => {
            if let Some(parent) = config.db_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating database directory {}", parent.display()))?;
            }
            GraphStore::open_sqlite(&config.db_path).context("opening sqlite graph store")?
        }
    };
    store
        .init_schema()
        .context("initializing graph store schema")?;

    let worker = ResearchWorker {
        uv_bin: config.uv_bin.clone(),
        python_dir: config.python_dir.clone(),
        fixture_dir: config.fixture_dir.clone(),
        timeout: config.worker_timeout,
    };

    let state = AppState {
        store: Arc::new(store),
        worker: Arc::new(worker),
    };

    let db_engine_label = match config.db_engine {
        DbEngine::Memory => "mem",
        DbEngine::Sqlite => "sqlite",
    };
    tracing::info!(
        bind = %config.bind,
        db_engine = db_engine_label,
        db_path = %config.db_path.display(),
        "starting iw-server"
    );

    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding to {}", config.bind))?;
    axum::serve(listener, router(state))
        .await
        .context("serving requests")?;

    Ok(())
}
