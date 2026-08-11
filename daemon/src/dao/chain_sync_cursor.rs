use thiserror::Error;

use crate::types::{
    ChainSyncCursor,
    ChainSyncCursorRow,
    ChainType,
};

use super::DaoExecutor;

#[derive(Debug, Error)]
pub enum DaoChainSyncCursorError {
    /// Database operation failed
    #[error("Database error during chain sync cursor operation")]
    DatabaseError,

    /// Stored cursor cannot be represented as a block number
    #[error("Stored chain sync cursor is invalid")]
    InvalidCursor,
}

pub trait DaoChainSyncCursorMethods: DaoExecutor + 'static {
    /// Read the sweep watermark for a chain. `None` means the sweep has never
    /// run against this database — the caller decides where to start.
    async fn get_chain_sync_cursor(
        &self,
        chain: ChainType,
    ) -> Result<Option<ChainSyncCursor>, DaoChainSyncCursorError> {
        let query = sqlx::query_as::<_, ChainSyncCursorRow>(
            "SELECT chain, last_processed_block, updated_at
             FROM chain_sync_cursors
             WHERE chain = ?",
        )
        .bind(chain);

        let row: Option<ChainSyncCursorRow> = self
            .fetch_optional(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.chain_sync_cursor",
                    error.operation = "get_chain_sync_cursor",
                    error.source = ?e,
                    %chain,
                    "Failed to read chain sync cursor"
                );

                DaoChainSyncCursorError::DatabaseError
            })?;

        row.map(ChainSyncCursor::try_from)
            .transpose()
            .map_err(|e| {
                tracing::error!(
                    error.category = "dao.chain_sync_cursor",
                    error.operation = "get_chain_sync_cursor",
                    error.source = %e,
                    %chain,
                    "Stored chain sync cursor is out of range, refusing to use it"
                );

                DaoChainSyncCursorError::InvalidCursor
            })
    }

    /// Move the watermark forward, and return the cursor as it now stands.
    ///
    /// A lower `block_number` is not an error and not a rollback: endpoints
    /// rotate and public RPC pools contain lagging nodes, so an older head is
    /// an ordinary observation. `MAX` in the conflict clause keeps the stored
    /// value monotonic regardless of what the caller passes, and `updated_at`
    /// only moves when the cursor actually advances, so it stays a truthful
    /// answer to "is the sweep making progress?".
    async fn advance_chain_sync_cursor(
        &self,
        chain: ChainType,
        block_number: u64,
    ) -> Result<ChainSyncCursor, DaoChainSyncCursorError> {
        let block_number = i64::try_from(block_number).map_err(|_e| {
            tracing::error!(
                error.category = "dao.chain_sync_cursor",
                error.operation = "advance_chain_sync_cursor",
                %chain,
                block_number,
                "Block number exceeds what SQLite can store, refusing to advance the cursor"
            );

            DaoChainSyncCursorError::InvalidCursor
        })?;

        let query = sqlx::query_as::<_, ChainSyncCursorRow>(
            "INSERT INTO chain_sync_cursors (chain, last_processed_block, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(chain) DO UPDATE SET
                 last_processed_block =
                     MAX(excluded.last_processed_block, chain_sync_cursors.last_processed_block),
                 updated_at = CASE
                     WHEN excluded.last_processed_block > chain_sync_cursors.last_processed_block
                         THEN excluded.updated_at
                     ELSE chain_sync_cursors.updated_at
                 END
             RETURNING chain, last_processed_block, updated_at",
        )
        .bind(chain)
        .bind(block_number)
        .bind(chrono::Utc::now().naive_utc());

        let row: ChainSyncCursorRow = self
            .fetch_one(query)
            .await
            .map_err(|e| {
                tracing::debug!(
                    error.category = "dao.chain_sync_cursor",
                    error.operation = "advance_chain_sync_cursor",
                    error.source = ?e,
                    %chain,
                    block_number,
                    "Failed to advance chain sync cursor"
                );

                DaoChainSyncCursorError::DatabaseError
            })?;

        ChainSyncCursor::try_from(row).map_err(|e| {
            tracing::error!(
                error.category = "dao.chain_sync_cursor",
                error.operation = "advance_chain_sync_cursor",
                error.source = %e,
                %chain,
                "Stored chain sync cursor is out of range after an advance"
            );

            DaoChainSyncCursorError::InvalidCursor
        })
    }
}

impl<T: DaoExecutor + 'static> DaoChainSyncCursorMethods for T {}

#[cfg(test)]
mod tests {
    use crate::dao::create_test_dao;

    use super::*;

    #[tokio::test]
    async fn cursor_is_absent_until_first_advance() {
        let dao = create_test_dao().await;

        let cursor = dao
            .get_chain_sync_cursor(ChainType::Polygon)
            .await
            .expect("read succeeds");

        assert!(cursor.is_none());
    }

    #[tokio::test]
    async fn advance_creates_then_moves_the_cursor() {
        let dao = create_test_dao().await;

        let created = dao
            .advance_chain_sync_cursor(ChainType::Polygon, 100)
            .await
            .expect("first advance succeeds");
        assert_eq!(created.last_processed_block, 100);

        let moved = dao
            .advance_chain_sync_cursor(ChainType::Polygon, 112)
            .await
            .expect("second advance succeeds");
        assert_eq!(moved.last_processed_block, 112);

        let stored = dao
            .get_chain_sync_cursor(ChainType::Polygon)
            .await
            .expect("read succeeds")
            .expect("cursor exists");
        assert_eq!(stored.last_processed_block, 112);
    }

    #[tokio::test]
    async fn lower_block_number_does_not_move_the_cursor_back() {
        let dao = create_test_dao().await;

        dao.advance_chain_sync_cursor(ChainType::Polygon, 1000)
            .await
            .expect("first advance succeeds");

        // A lagging node in a public RPC pool reports an older head. Reported
        // as text this would compare '999' > '1000' and win.
        let after_lag = dao
            .advance_chain_sync_cursor(ChainType::Polygon, 999)
            .await
            .expect("stale advance is not an error");

        assert_eq!(after_lag.last_processed_block, 1000);
    }

    #[tokio::test]
    async fn no_op_advance_keeps_updated_at() {
        let dao = create_test_dao().await;

        let created = dao
            .advance_chain_sync_cursor(ChainType::Polygon, 1000)
            .await
            .expect("first advance succeeds");

        let after_lag = dao
            .advance_chain_sync_cursor(ChainType::Polygon, 500)
            .await
            .expect("stale advance is not an error");

        assert_eq!(after_lag.updated_at, created.updated_at);
    }

    #[tokio::test]
    async fn cursors_of_different_chains_are_independent() {
        let dao = create_test_dao().await;

        dao.advance_chain_sync_cursor(ChainType::Polygon, 1000)
            .await
            .expect("polygon advance succeeds");
        dao.advance_chain_sync_cursor(ChainType::PolkadotAssetHub, 7)
            .await
            .expect("asset hub advance succeeds");

        let polygon = dao
            .get_chain_sync_cursor(ChainType::Polygon)
            .await
            .expect("read succeeds")
            .expect("cursor exists");
        let asset_hub = dao
            .get_chain_sync_cursor(ChainType::PolkadotAssetHub)
            .await
            .expect("read succeeds")
            .expect("cursor exists");

        assert_eq!(polygon.last_processed_block, 1000);
        assert_eq!(asset_hub.last_processed_block, 7);
    }
}
