/// ## Data Access Object (DAO) for Kalatori application.
///
/// Please follow the architectural vision for the DAO methods:
/// - Keep methods focused on single responsibilities (e.g., create, read,
///   update). Don't implement any business logic here.
/// - All creation and update methods should return the full updated object.
/// - We manually update `updated_at` and increment `version` in UPDATE
///   statements rather than using database triggers.
/// - We want to be able to compare datetime fields directly in SQL queries,
///   so we convert `chrono::DateTime<Utc>` to `NaiveDateTime` when binding parameters
///   (see details [here](https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html#note-current_timestamp-and-comparisoninteroperability-of-datetime-values)).
mod changes;
mod error_parsing;
mod interface;
mod invoice;
mod payout;
mod refund;
mod swap;
mod transaction;
mod webhook_event;

use sqlx::{
    Executor,
    SqliteTransaction,
};
use tokio::sync::Mutex;

use crate::configs::DatabaseConfig;

// Export domain-specific errors
pub use changes::DaoChangesError;
pub use invoice::DaoInvoiceError;
#[expect(unused_imports)]
pub use payout::DaoPayoutError;
#[expect(unused_imports)]
pub use refund::DaoRefundError;
pub use swap::DaoSwapError;
pub use transaction::DaoTransactionError;
#[cfg_attr(not(test), expect(unused_imports))]
pub use webhook_event::DaoWebhookEventError;

// Export high-level interface traits
pub use interface::{
    DaoInterface,
    DaoTransactionInterface,
};

// Export mocks only in test builds
#[cfg(test)]
pub use interface::{
    MockDaoInterface,
    MockDaoTransactionInterface,
};

const SQLITE_FILE_NAME: &str = "kalatori_db.sqlite";

// Keep DaoResult for internal use (DaoExecutor trait methods)
pub(crate) type DaoResult<T> = Result<T, sqlx::Error>;

pub trait DaoExecutor: Send + Sync {
    async fn fetch_optional<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Option<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static;

    async fn fetch_one<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<R, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static;

    async fn fetch_all<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Vec<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static;
}

pub struct DaoTransaction {
    // Use `Mutex` to avoid mutability requirement in order to unify the API for both Transaction
    // and Pool Use `tokio::sync::Mutex` cause `std::sync::Mutex` is not `Send`
    transaction: Mutex<SqliteTransaction<'static>>,
}

impl DaoTransaction {
    pub async fn commit(self) -> DaoResult<()> {
        let lock = self.transaction.into_inner();
        lock.commit().await
    }

    #[expect(dead_code)]
    pub async fn rollback(self) -> DaoResult<()> {
        let lock = self.transaction.into_inner();
        lock.rollback().await
    }
}

impl DaoExecutor for DaoTransaction {
    async fn fetch_optional<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Option<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        let mut lock = self.transaction.lock().await;
        let result = (&mut **lock)
            .fetch_optional(query)
            .await?;

        if let Some(row) = result {
            O::from_row(&row)
                .map(From::from)
                .map(Some)
        } else {
            Ok(None)
        }
    }

    async fn fetch_one<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<R, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        let mut lock = self.transaction.lock().await;
        (&mut **lock)
            .fetch_one(query)
            .await
            .and_then(|row| O::from_row(&row).map(From::from))
    }

    async fn fetch_all<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Vec<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        let mut lock = self.transaction.lock().await;
        (&mut **lock)
            .fetch_all(query)
            .await?
            .into_iter()
            .map(|row| O::from_row(&row).map(From::from))
            .collect()
    }
}

#[expect(clippy::upper_case_acronyms)]
#[derive(Clone)]
pub struct DAO {
    pool: sqlx::SqlitePool,
}

impl DAO {
    pub async fn new(config: DatabaseConfig) -> DaoResult<Self> {
        let (pool_options, connection_options) = if config.temporary {
            tracing::info!("Using in-memory temporary database");
            let pool_opts = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1);
            let conn_opts = sqlx::sqlite::SqliteConnectOptions::new()
                .create_if_missing(true)
                .in_memory(true);
            (pool_opts, conn_opts)
        } else {
            if !std::fs::exists(&config.dir)? {
                std::fs::create_dir_all(&config.dir)?;
                tracing::warn!(
                    "Failed to find sqlite3 database directory at {}. Created new directory at {} with database file {} inside.",
                    config.dir,
                    config.dir,
                    SQLITE_FILE_NAME,
                )
            }
            let pool_opts = sqlx::sqlite::SqlitePoolOptions::new();
            let conn_opts = sqlx::sqlite::SqliteConnectOptions::new()
                .create_if_missing(true)
                .filename(format!(
                    "{}/{}",
                    config.dir, SQLITE_FILE_NAME,
                ));
            (pool_opts, conn_opts)
        };

        let pool = pool_options
            .connect_with(connection_options)
            .await
            .expect("Failed to create database connection pool");

        let dao = Self {
            pool,
        };

        let sqlite_version = dao.sqlite_version().await?;
        tracing::info!(
            "Current SQLite version: {}",
            sqlite_version
        );

        tracing::info!("Run database migrations...");

        sqlx::migrate!("../migrations")
            .run(&dao.pool)
            .await?;

        tracing::info!("Database migrations done.");

        Ok(dao)
    }

    pub async fn begin_transaction(&self) -> DaoResult<DaoTransaction> {
        let transaction = self.pool.begin().await?;

        Ok(DaoTransaction {
            transaction: Mutex::new(transaction),
        })
    }

    pub async fn sqlite_version(&self) -> DaoResult<String> {
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await?;

        Ok(version)
    }
}

impl DaoExecutor for DAO {
    async fn fetch_optional<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Option<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        let result = self.pool.fetch_optional(query).await?;

        if let Some(row) = result {
            O::from_row(&row)
                .map(From::from)
                .map(Some)
        } else {
            Ok(None)
        }
    }

    async fn fetch_one<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<R, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        self.pool
            .fetch_one(query)
            .await
            .and_then(|row| O::from_row(&row).map(From::from))
    }

    async fn fetch_all<'a, O, R>(
        &self,
        query: sqlx::query::QueryAs<'a, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'a>>,
    ) -> Result<Vec<R>, sqlx::Error>
    where
        O: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin + 'static,
        R: From<O>,
        Self: 'static,
    {
        self.pool
            .fetch_all(query)
            .await?
            .into_iter()
            .map(|row| O::from_row(&row).map(From::from))
            .collect()
    }
}

#[cfg(test)]
async fn create_test_dao() -> DAO {
    use crate::configs::DatabaseConfig;

    let config = DatabaseConfig {
        dir: String::new(),
        temporary: true,
    };

    DAO::new(config)
        .await
        .expect("Failed to create test DAO")
}
