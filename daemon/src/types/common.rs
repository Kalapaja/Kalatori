//! Common types shared across multiple modules

use chrono::{
    DateTime,
    Utc,
};
use kalatori_client::strum::{
    Display,
    EnumString,
};
use rust_decimal::Decimal;
use serde::{
    Deserialize,
    Serialize,
};
use sqlx::types::Text;
use sqlx::{
    FromRow,
    Type,
};

pub use kalatori_client::types::ChainType;

/// Initiator type for payouts and refunds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Display, EnumString)]
#[strum(crate = "kalatori_client::strum")]
pub enum InitiatorType {
    System,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferInfo {
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Decimal,
    pub source_address: String,
    pub destination_address: String,
}

#[derive(FromRow)]
pub struct TransferInfoRow {
    pub chain: ChainType,
    pub asset_id: String,
    pub asset_name: String,
    pub amount: Text<Decimal>,
    pub source_address: String,
    pub destination_address: String,
}

impl From<TransferInfoRow> for TransferInfo {
    fn from(value: TransferInfoRow) -> Self {
        Self {
            chain: value.chain,
            asset_id: value.asset_id,
            asset_name: value.asset_name,
            amount: value.amount.into_inner(),
            source_address: value.source_address,
            destination_address: value.destination_address,
        }
    }
}

/// Retry metadata for payouts and refunds
#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct RetryMeta {
    pub retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
}

impl RetryMeta {
    fn retry_delay_secs(&self) -> i64 {
        // TODO: it's simplified strategy. In future might be better
        // to calculate delay based on average block execution time of the chain
        match self.retry_count {
            0 => 60,          // 1 minute
            1 => 5 * 60,      // 5 minutes
            2 => 15 * 60,     // 15 minutes
            3 => 30 * 60,     // 30 minutes
            4 => 60 * 60,     // 1 hour
            _ => 2 * 60 * 60, // 2 hours
        }
    }

    pub fn increment_retry(
        &mut self,
        failure_message: String,
    ) {
        let now = Utc::now();
        // `retry_delay_secs` caps at 2 hours once the count passes 4, so
        // saturating the counter keeps the backoff at its cap rather than
        // wrapping it back into the 1-minute bucket.
        self.retry_count = self.retry_count.saturating_add(1);
        self.last_attempt_at = Some(now);

        #[expect(
            clippy::arithmetic_side_effects,
            reason = "`retry_delay_secs` returns one of six fixed values, at most 2 hours; adding that to `Utc::now()` cannot overflow DateTime<Utc>"
        )]
        let next_retry_at = now + chrono::Duration::seconds(self.retry_delay_secs());

        self.next_retry_at = Some(next_retry_at);
        self.failure_message = Some(failure_message);
    }

    #[cfg(test)]
    pub fn trunc_timestamps(&mut self) {
        use chrono::SubsecRound;

        self.last_attempt_at = self
            .last_attempt_at
            .map(|dt| dt.trunc_subsecs(0));
        self.next_retry_at = self
            .next_retry_at
            .map(|dt| dt.trunc_subsecs(0));
    }
}

// ── Pagination & sorting ─────────────────────────────────────────────

const DEFAULT_PAGE: u32 = 1;
const DEFAULT_PER_PAGE: u32 = 20;
const MAX_PER_PAGE: u32 = 100;

/// Sort direction for list queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

impl SortOrder {
    pub fn as_sql(&self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// Pagination parameters extracted from query string.
#[serde_with::serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaginationParams {
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub page: Option<u32>,
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub per_page: Option<u32>,
}

impl PaginationParams {
    pub fn validated_page(&self) -> u32 {
        self.page.unwrap_or(DEFAULT_PAGE).max(1)
    }

    pub fn validated_per_page(&self) -> u32 {
        self.per_page
            .unwrap_or(DEFAULT_PER_PAGE)
            .clamp(1, MAX_PER_PAGE)
    }

    /// SQL `OFFSET` for the requested page.
    ///
    /// `page` is an unvalidated query-string parameter, so `(page - 1) *
    /// per_page` overflows `u32` for any `page` above ~43 million. That used to
    /// wrap silently (serving an arbitrary wrong page) and, now that release
    /// builds actually enable `overflow-checks`, would panic the handler.
    /// Saturating yields an empty page, which is the right answer for a page
    /// number that far past the end of the table.
    pub fn offset(&self) -> u32 {
        self.validated_page()
            .saturating_sub(1)
            .saturating_mul(self.validated_per_page())
    }
}

/// Paginated response wrapper for list endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    pub total_pages: u32,
}

impl<T: Serialize> PaginatedResponse<T> {
    pub fn new(
        items: Vec<T>,
        total: u32,
        page: u32,
        per_page: u32,
    ) -> Self {
        let total_pages = if per_page == 0 {
            0
        } else {
            total.div_ceil(per_page)
        };

        Self {
            items,
            total,
            page,
            per_page,
            total_pages,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> PaginationParams {
        PaginationParams {
            page,
            per_page,
        }
    }

    #[test]
    fn offset_defaults_to_the_first_page() {
        assert_eq!(params(None, None).offset(), 0);
        assert_eq!(params(Some(1), Some(25)).offset(), 0);
    }

    #[test]
    fn offset_scales_with_page_number() {
        assert_eq!(params(Some(3), Some(25)).offset(), 50);
    }

    #[test]
    fn offset_saturates_instead_of_wrapping_on_a_hostile_page_number() {
        // `page` is an unvalidated query-string parameter. `(u32::MAX - 1) *
        // 100` overflows: it used to wrap to a small, arbitrary offset and,
        // with `overflow-checks` now actually enabled in release builds, would
        // panic the handler. Saturating gives an empty page.
        assert_eq!(
            params(Some(u32::MAX), Some(MAX_PER_PAGE)).offset(),
            u32::MAX
        );
        assert_eq!(
            params(Some(u32::MAX), Some(1)).offset(),
            u32::MAX.saturating_sub(1)
        );
    }

    #[test]
    fn increment_retry_saturates_the_counter() {
        let mut meta = RetryMeta {
            retry_count: u32::MAX,
            ..RetryMeta::default()
        };
        meta.increment_retry("boom".to_string());

        // Must stay at the cap rather than wrapping back to 0, which would
        // reset the exponential backoff to its shortest delay.
        assert_eq!(meta.retry_count, u32::MAX);
        assert!(meta.next_retry_at.is_some());
    }
}
