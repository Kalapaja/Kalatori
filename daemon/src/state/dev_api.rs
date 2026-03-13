use std::collections::HashMap;

use uuid::Uuid;

use crate::dao::DaoInterface;
use crate::types::InvoiceWithReceivedAmount;

use super::AppState;

impl<D: DaoInterface> AppState<D> {
    pub async fn get_invoices_registry_state(&self) -> HashMap<Uuid, InvoiceWithReceivedAmount> {
        self.registry.state().await
    }
}
