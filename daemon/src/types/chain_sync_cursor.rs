use chrono::{
    DateTime,
    Utc,
};
use thiserror::Error;

use super::ChainType;

/// How far the catch-up sweep has processed a chain.
///
/// The sweep re-reads logs the live subscription may have missed, so this
/// watermark has to survive a daemon restart — see issue #333.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainSyncCursor {
    pub chain: ChainType,
    /// Highest block whose transfers are known to be recorded.
    pub last_processed_block: u64,
    /// When the cursor last moved forward. Unchanged by a no-op advance, so it
    /// answers "is the sweep making progress?".
    pub updated_at: DateTime<Utc>,
}

/// Database representation of [`ChainSyncCursor`].
///
/// SQLite stores integers as signed 64-bit, while block numbers are unsigned,
/// so the column type and the domain type cannot be the same.
#[derive(Debug, sqlx::FromRow)]
pub struct ChainSyncCursorRow {
    pub chain: ChainType,
    pub last_processed_block: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("Chain sync cursor for {chain} holds a negative block number: {last_processed_block}")]
pub struct NegativeCursorError {
    pub chain: ChainType,
    pub last_processed_block: i64,
}

impl TryFrom<ChainSyncCursorRow> for ChainSyncCursor {
    type Error = NegativeCursorError;

    fn try_from(row: ChainSyncCursorRow) -> Result<Self, Self::Error> {
        // The table has `CHECK(last_processed_block >= 0)`, so this only fires
        // for a database written by something other than this daemon.
        let last_processed_block =
            u64::try_from(row.last_processed_block).map_err(|_e| NegativeCursorError {
                chain: row.chain,
                last_processed_block: row.last_processed_block,
            })?;

        Ok(Self {
            chain: row.chain,
            last_processed_block,
            updated_at: row.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_converts_to_cursor() {
        let row = ChainSyncCursorRow {
            chain: ChainType::Polygon,
            last_processed_block: 80_000_000,
            updated_at: Utc::now(),
        };
        let updated_at = row.updated_at;

        let cursor = ChainSyncCursor::try_from(row).expect("non-negative block number converts");

        assert_eq!(cursor.chain, ChainType::Polygon);
        assert_eq!(cursor.last_processed_block, 80_000_000);
        assert_eq!(cursor.updated_at, updated_at);
    }

    #[test]
    fn negative_block_number_is_rejected() {
        let row = ChainSyncCursorRow {
            chain: ChainType::Polygon,
            last_processed_block: -1,
            updated_at: Utc::now(),
        };

        assert_eq!(
            ChainSyncCursor::try_from(row),
            Err(NegativeCursorError {
                chain: ChainType::Polygon,
                last_processed_block: -1,
            })
        );
    }
}
