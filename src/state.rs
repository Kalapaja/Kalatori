use crate::error::ForceWithdrawalError;
use crate::{
    chain::ChainManager,
    database::{ConfigWoChains, Database, TransactionInfoDb},
    definitions::api_v2::{
        CurrencyProperties, Health, OrderCreateResponse, OrderInfo, OrderQuery, OrderResponse,
        OrderStatus, RpcInfo, ServerHealth, ServerInfo, ServerStatus,
    },
    error::{Error, OrderError},
    signer::Signer,
    utils::task_tracker::TaskTracker,
};
use std::collections::HashMap;
use substrate_crypto_light::common::{AccountId32, AsBase58};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Struct to store state of daemon. If something requires cooperation of more than one component,
/// it should go through here.
#[derive(Clone, Debug)]
pub struct State {
    tx: tokio::sync::mpsc::Sender<StateAccessRequest>,
}

impl State {
    #[expect(clippy::too_many_lines)]
    pub fn initialise(
        signer: Signer,
        ConfigWoChains {
            recipient,
            debug,
            remark,
        }: ConfigWoChains,
        db: Database,
        chain_manager_receiver: oneshot::Receiver<ChainManager>,
        instance_id: String,
        task_tracker: TaskTracker,
        shutdown_notification: CancellationToken,
    ) -> Self {
        /*
            currencies: HashMap<String, CurrencyProperties>,
            recipient: AccountId,
            pair: Pair,
            depth: Option<Timestamp>,
            account_lifetime: Timestamp,
            debug: bool,
            remark: String,
            invoices: RwLock<HashMap<String, Invoicee>>,
            rpc: String,
        */
        let (tx, mut rx) = tokio::sync::mpsc::channel(1024);

        let server_info = ServerInfo {
            // TODO
            version: env!("CARGO_PKG_VERSION").to_string(),
            debug: debug.unwrap_or_default(),
            kalatori_remark: remark,
            instance_id,
        };

        // Remember to always spawn async here or things might deadlock
        task_tracker.clone().spawn("State Handler", async move {
            let chain_manager = chain_manager_receiver.await.map_err(|_| Error::Fatal)?;
            let db_wakeup = db.clone();
            let chain_manager_wakeup = chain_manager.clone();
            let currencies = HashMap::new();
            let mut state = StateData {
                currencies,
                recipient,
                server_info,
                db,
                chain_manager,
                signer,
            };

            // TODO: consider doing this even more lazy
            let order_list = db_wakeup.order_list().await?;
            task_tracker.spawn("Restore saved orders", async move {
                for (order, order_details) in order_list {
                    // TODO: handle error?
                    drop(chain_manager_wakeup
                        .add_invoice(order, order_details, state.recipient)
                        .await);
                }
                Ok("All saved orders restored")
            });

            loop {
                tokio::select! {
                    biased;
                    request_option = rx.recv() => {
                        let Some(state_request) = request_option else {
                            break;
                        };

                        match state_request {
                            StateAccessRequest::ConnectChain(assets) => {
                                // it MUST be asserted in chain tracker that assets are those and only
                                // those that user requested
                                state.update_currencies(assets);
                            }
                            StateAccessRequest::GetInvoiceStatus(request) => {
                                request
                                    .res
                                    .send(state.get_invoice_status(request.order).await)
                                    .map_err(|_| Error::Fatal)?;
                            }
                            StateAccessRequest::CreateInvoice(request) => {
                                request
                                    .res
                                    .send(state.create_invoice(request.order_query).await)
                                    .map_err(|_| Error::Fatal)?;
                            }
                            StateAccessRequest::IsCurrencySupported { currency, res } => {
                                let supported = state.currencies.contains_key(&currency);
                                res.send(supported).map_err(|_| Error::Fatal)?;
                            }
                            StateAccessRequest::ServerStatus(res) => {
                                let server_status = ServerStatus {
                                    server_info: state.server_info.clone(),
                                    supported_currencies: state.currencies.clone(),
                                };
                                res.send(server_status).map_err(|_| Error::Fatal)?;
                            }
                            StateAccessRequest::ServerHealth(res) => {
                                let connected_rpcs = state.chain_manager.get_connected_rpcs().await?;
                                let server_health = ServerHealth {
                                    server_info: state.server_info.clone(),
                                    connected_rpcs: connected_rpcs.clone(),
                                    status: Self::overall_health(&connected_rpcs),
                                };
                                res.send(server_health).map_err(|_| Error::Fatal)?;
                            }
                            StateAccessRequest::OrderPaid(id) => {
                                // Only perform actions if the record is saved in ledger
                                let marked = state.db.mark_paid(id.clone()).await;

                                match marked {
                                    Ok(order) => {
                                        if !order.callback.is_empty() {
                                            let callback = order.callback.clone();
                                            tokio::spawn(async move {
                                                use reqwest::{Client, redirect::Policy};
                                                use std::time::Duration;

                                                tracing::info!("Sending callback to: {}", callback);

                                                let client_res = Client::builder()
                                                    // No redirects here to prevent different attach vectors.
                                                    // Every redirect must be validated separately.
                                                    .redirect(Policy::none())
                                                    .timeout(Duration::from_secs(10))
                                                    .build();

                                                match client_res {
                                                    Ok(client) => {
                                                        // Fire and forget.
                                                        // Note: If response is read, always check payload size!
                                                        if let Err(e) = client.get(&callback).send().await {
                                                            tracing::error!("Failed to send callback to {callback}: {e:?}");
                                                        }
                                                    }
                                                    Err(e) => tracing::error!("Failed to build HTTP client for callback: {e:?}"),
                                                }
                                            });
                                        }
                                        drop(state.chain_manager.reap(id, order, state.recipient).await);
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Order was paid but this could not be recorded! {e:?}"
                                        );
                                    }
                                }
                            }
                            StateAccessRequest::RecordTransaction { order, tx: new_tx } => {
                                let record = state.db.record_transaction(order, new_tx).await;

                                if let Err(e) = record {
                                    tracing::error!(
                                        "Found a transaction related to an order, but this could not be recorded! {e:?}"
                                    );
                                }
                            }
                            StateAccessRequest::OrderWithdrawn(id) => {
                                match state.db.mark_withdrawn(id.clone()).await {
                                    Ok(_order) => {
                                        tracing::info!("Order {id} successfully marked as withdrawn");
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Order was withdrawn but this could not be recorded! {e:?}"
                                        );
                                    }
                                };
                            }
                            StateAccessRequest::ForceWithdrawal(id) => {
                                let order = state.db.read_order(id.clone()).await;

                                match order {
                                    Ok(Some(order_info)) => {
                                        let result = state.chain_manager.reap(id.clone(), order_info, state.recipient).await;

                                        match result {
                                            Ok(()) => {
                                                let marked = state.db.mark_forced(id.clone()).await;

                                                match marked {
                                                    Ok(()) => {
                                                        tracing::info!("Order {id} successfully marked as force withdrawn");
                                                    }
                                                    Err(e) => {
                                                        tracing::error!("Failed to mark order {id} as forced: {e:?}");
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!("Failed to initiate forced payout for order {id}: {e:?}");
                                            }
                                        }
                                    }
                                    Ok(None) => {
                                        tracing::error!("Order {id} not found in database");
                                    }
                                    Err(e) => {
                                        tracing::error!("Error reading order {id} from database: {e:?}");
                                    }
                                }
                            }
                            StateAccessRequest::IsOrderPaid(id, res) => {
                                let is_marked_paid = state.db.is_marked_paid(id).await;

                                match is_marked_paid {
                                    Ok(paid) => {
                                        res.send(paid).map_err(|_| Error::Fatal)?;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to read the order state! {e:?}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    // Orchestrate shutdown from here
                    () = shutdown_notification.cancelled() => {
                        // Web server shuts down on its own; it does not matter what it sends now.

                        // First shut down active actions for external world. If something yet
                        // happens, we should record it in db.
                        state.chain_manager.shutdown().await;

                        // Now that nothing happens we can wind down the ledger
                        state.db.shutdown().await;

                        // Try to zeroize secrets
                        state.signer.shutdown().await;

                        // And shut down finally
                        break;
                    }
                }
            }

            Ok("State handler is shutting down")
        });

        Self { tx }
    }
    fn overall_health(connected_rpcs: &[RpcInfo]) -> Health {
        if connected_rpcs.iter().all(|rpc| rpc.status == Health::Ok) {
            Health::Ok
        } else if connected_rpcs.iter().any(|rpc| rpc.status == Health::Ok) {
            Health::Degraded
        } else {
            Health::Critical
        }
    }

    pub async fn connect_chain(&self, assets: HashMap<String, CurrencyProperties>) {
        self.tx
            .send(StateAccessRequest::ConnectChain(assets))
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Failed to send ConnectChain request: {}", e);
            });
    }

    pub async fn order_status(&self, order: &str) -> Result<OrderResponse, Error> {
        let (res, rx) = oneshot::channel();
        self.tx
            .send(StateAccessRequest::GetInvoiceStatus(GetInvoiceStatus {
                order: order.to_string(),
                res,
            }))
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)?
    }

    pub async fn server_status(&self) -> Result<ServerStatus, Error> {
        let (res, rx) = oneshot::channel();
        self.tx
            .send(StateAccessRequest::ServerStatus(res))
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)
    }

    pub async fn server_health(&self) -> Result<ServerHealth, Error> {
        let (res, rx) = oneshot::channel();
        self.tx
            .send(StateAccessRequest::ServerHealth(res))
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)
    }

    pub async fn create_order(&self, order_query: OrderQuery) -> Result<OrderResponse, Error> {
        let (res, rx) = oneshot::channel();
        /*
                Invoicee {
                        callback: callback.clone(),
                        amount: Balance::parse(amount, 6),
                        paid: false,
                        paym_acc: pay_acc.clone(),
                    },
        */
        self.tx
            .send(StateAccessRequest::CreateInvoice(CreateInvoice {
                order_query,
                res,
            }))
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)?
    }

    pub async fn is_currency_supported(&self, currency: &str) -> Result<bool, Error> {
        let (res, rx) = oneshot::channel();
        self.tx
            .send(StateAccessRequest::IsCurrencySupported {
                currency: currency.to_string(),
                res,
            })
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)
    }

    pub async fn is_order_paid(&self, order: String) -> Result<bool, Error> {
        let (res, rx) = oneshot::channel();
        self.tx
            .send(StateAccessRequest::IsOrderPaid(order, res))
            .await
            .map_err(|_| Error::Fatal)?;
        rx.await.map_err(|_| Error::Fatal)
    }

    pub async fn order_paid(&self, order: String) {
        if self
            .tx
            .send(StateAccessRequest::OrderPaid(order))
            .await
            .is_err()
        {
            tracing::warn!("Data race on shutdown; please restart the daemon for cleaning up");
        }
    }

    pub async fn order_withdrawn(&self, order: String) {
        if self
            .tx
            .send(StateAccessRequest::OrderWithdrawn(order))
            .await
            .is_err()
        {
            tracing::warn!("Data race on shutdown; please restart the daemon for cleaning up");
        }
    }

    pub async fn force_withdrawal(
        &self,
        order: String,
    ) -> Result<OrderResponse, ForceWithdrawalError> {
        self.tx
            .send(StateAccessRequest::ForceWithdrawal(order.clone()))
            .await
            .map_err(|_| ForceWithdrawalError::InvalidParameter(order.clone()))?;

        match self.order_status(&order).await {
            Ok(order_status) => Ok(order_status),
            Err(_) => Ok(OrderResponse::NotFound),
        }
    }
    pub fn interface(&self) -> Self {
        State {
            tx: self.tx.clone(),
        }
    }

    pub async fn record_transaction(
        &self,
        tx: TransactionInfoDb,
        order: String,
    ) -> Result<(), Error> {
        self.tx
            .send(StateAccessRequest::RecordTransaction { order, tx })
            .await
            .map_err(|_| Error::Fatal)
    }
}

enum StateAccessRequest {
    ConnectChain(HashMap<String, CurrencyProperties>),
    GetInvoiceStatus(GetInvoiceStatus),
    CreateInvoice(CreateInvoice),
    IsCurrencySupported {
        currency: String,
        res: oneshot::Sender<bool>,
    },
    ServerStatus(oneshot::Sender<ServerStatus>),
    ServerHealth(oneshot::Sender<ServerHealth>),
    OrderPaid(String),
    IsOrderPaid(String, oneshot::Sender<bool>),
    RecordTransaction {
        order: String,
        tx: TransactionInfoDb,
    },
    OrderWithdrawn(String),
    ForceWithdrawal(String),
}

struct GetInvoiceStatus {
    pub order: String,
    pub res: oneshot::Sender<Result<OrderResponse, Error>>,
}

struct CreateInvoice {
    pub order_query: OrderQuery,
    pub res: oneshot::Sender<Result<OrderResponse, Error>>,
}

struct StateData {
    currencies: HashMap<String, CurrencyProperties>,
    recipient: AccountId32,
    server_info: ServerInfo,
    db: Database,
    chain_manager: ChainManager,
    signer: Signer,
}

impl StateData {
    fn update_currencies(&mut self, currencies: HashMap<String, CurrencyProperties>) {
        self.currencies.extend(currencies);
    }

    async fn get_invoice_status(&self, order: String) -> Result<OrderResponse, Error> {
        if let Some(order_info) = self.db.read_order(order.clone()).await? {
            let message = String::new(); //TODO
            Ok(OrderResponse::FoundOrder(OrderStatus {
                order,
                message,
                recipient: self.recipient.clone().to_base58_string(2), // TODO maybe but spec says use "2"
                server_info: self.server_info.clone(),
                order_info,
                payment_page: String::new(),
                redirect_url: String::new(),
            }))
        } else {
            Ok(OrderResponse::NotFound)
        }
    }

    async fn create_invoice(&self, order_query: OrderQuery) -> Result<OrderResponse, Error> {
        let order = order_query.order.clone();
        let currency_properties = self
            .currencies
            .get(&order_query.currency)
            .ok_or(OrderError::UnknownCurrency)?;
        let currency = currency_properties.info(order_query.currency.clone());
        let payment_account = self.signer.public(order.clone(), currency.ss58).await?;
        match self
            .db
            .create_order(order.clone(), order_query, currency, payment_account)
            .await?
        {
            OrderCreateResponse::New(new_order_info) => {
                self.chain_manager
                    .add_invoice(order.clone(), new_order_info.clone(), self.recipient)
                    .await?;
                Ok(OrderResponse::NewOrder(self.order_status(
                    order,
                    new_order_info,
                    String::new(),
                )))
            }
            OrderCreateResponse::Modified(order_info) => Ok(OrderResponse::ModifiedOrder(
                self.order_status(order, order_info, String::new()),
            )),
            OrderCreateResponse::Collision(order_status) => {
                Ok(OrderResponse::CollidedOrder(self.order_status(
                    order,
                    order_status,
                    String::from("Order with this ID was already processed"),
                )))
            }
        }
    }

    fn order_status(&self, order: String, order_info: OrderInfo, message: String) -> OrderStatus {
        OrderStatus {
            order,
            message,
            recipient: self.recipient.clone().to_base58_string(2), // TODO maybe but spec says use "2"
            server_info: self.server_info.clone(),
            order_info,
            payment_page: String::new(),
            redirect_url: String::new(),
        }
    }
}
