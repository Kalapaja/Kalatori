use std::collections::HashSet;
use std::pin::Pin;

use futures::stream::{
    FuturesUnordered,
    StreamExt,
};
use tokio::time::{
    Duration,
    interval,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use kalatori_client::utils::{
    HmacConfig,
    add_headers_to_reqwest,
};

use crate::dao::DaoInterface;
use crate::types::WebhookEvent;

const WEBHOOK_SENDER_INTERVAL_MILLIS: u64 = 100;
const WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS: usize = 10;
const WEBHOOK_SENDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq, Eq)]
struct SendWebhookResult {
    event_id: Uuid,
    is_ok: bool,
}

#[tracing::instrument(skip(client, request))]
async fn send_webhook(
    client: reqwest::Client,
    request: reqwest::Request,
    event_id: Uuid,
) -> SendWebhookResult {
    match client.execute(request).await {
        Ok(response) if response.status().is_success() => {
            tracing::debug!(
                event_id = %event_id,
                "Successfully sent webhook event"
            );

            SendWebhookResult {
                event_id,
                is_ok: true,
            }
        },
        Ok(response) => {
            let status = response.status();
            let response_text = response.text().await;

            tracing::warn!(
                event_id = %event_id,
                response.status = %status,
                response.text = ?response_text,
                "Failed to send webhook event, non-success status code received",
            );
            SendWebhookResult {
                event_id,
                is_ok: false,
            }
        },
        Err(e) => {
            tracing::warn!(
                event_id = %event_id,
                error = %e,
                "Failed to send webhook event, request error occurred",
            );

            SendWebhookResult {
                event_id,
                is_ok: false,
            }
        },
    }
}

pub struct WebhookSender<D: DaoInterface + 'static> {
    client: reqwest::Client,
    dao: D,
    webhook_url: String,
    hmac_config: HmacConfig,
    processing_events_ids: HashSet<Uuid>,
}

impl<D: DaoInterface + 'static> WebhookSender<D> {
    pub fn new(
        dao: D,
        webhook_url: String,
        hmac_config: HmacConfig,
    ) -> Self {
        WebhookSender {
            client: reqwest::Client::new(),
            dao,
            webhook_url,
            hmac_config,
            processing_events_ids: HashSet::new(),
        }
    }

    fn build_request(
        &self,
        event: WebhookEvent,
    ) -> reqwest::Request {
        let mut request = self
            .client
            .post(&self.webhook_url)
            .json(&event.payload)
            .timeout(WEBHOOK_SENDER_REQUEST_TIMEOUT)
            .build()
            // This can fail only if we have invalid URL or serialization fails.
            // So we need to check URL on startup. Don't expect serialization failures.
            .inspect_err(|e| {
                tracing::error!(
                    error.source = ?e,
                    "Error while building webhook event request"
                )
            })
            // TODO: Normally this shouldn't fail at all, but we don't check URL validity on startup
            // for now
            .unwrap();

        add_headers_to_reqwest(&self.hmac_config, &mut request);

        request
    }

    fn build_future(
        &self,
        event: WebhookEvent,
    ) -> Pin<Box<dyn Future<Output = SendWebhookResult> + Send + 'static>> {
        let event_id = event.id;
        let request = self.build_request(event);

        Box::pin(send_webhook(
            self.client.clone(),
            request,
            event_id,
        ))
    }

    async fn prepare_webhook_events(
        &mut self
    ) -> Vec<Pin<Box<dyn Future<Output = SendWebhookResult> + Send + 'static>>> {
        let limit = WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS - self.processing_events_ids.len();

        if limit == 0 {
            return Vec::new();
        }

        let events = self
            .dao
            .get_webhook_events_to_send(u32::try_from(limit).unwrap_or_default())
            .await
            .inspect_err(|_| {
                tracing::warn!(
                    error.category = "webhook_sender",
                    error.operation = "prepare_webhook_events",
                    "Failed to fetch pending webhook events from database"
                );
            })
            .unwrap_or_default();

        events
            .into_iter()
            .filter_map(|event| {
                self.processing_events_ids
                    .insert(event.id)
                    .then_some(self.build_future(event))
            })
            .collect()
    }

    async fn handle_send_webhook_result(
        &mut self,
        result: SendWebhookResult,
    ) {
        self.processing_events_ids
            .remove(&result.event_id);

        if result.is_ok
            && self
                .dao
                .mark_webhook_event_as_sent(result.event_id)
                .await
                .is_err()
        {
            tracing::warn!(
                event_id = %result.event_id,
                error.category = "webhook_sender",
                error.operation = "handle_send_webhook_result",
                "Failed to mark webhook event as sent in database. It might be resent"
            )
        };
        // TODO: for now we do nothing on failure, the event will be retried
        // later. Later we might want to implement some retry strategy
        // with backoff and max attempts count
    }

    async fn perform(
        mut self,
        token: CancellationToken,
    ) {
        let mut interval = interval(Duration::from_millis(
            WEBHOOK_SENDER_INTERVAL_MILLIS,
        ));

        let mut shutdown_expected = false;
        let mut futures_set = FuturesUnordered::new();

        loop {
            tokio::select! {
                _ = interval.tick(), if !shutdown_expected => {
                    futures_set.extend(self.prepare_webhook_events().await);
                }
                future_result = futures_set.next(), if !futures_set.is_empty() => {
                    if let Some(data) = future_result {
                        self.handle_send_webhook_result(data).await;
                    }

                    if futures_set.is_empty() && shutdown_expected {
                        tracing::info!(
                            "All pending tasks finished, webhook sender is shutting down"
                        );

                        break;
                    }
                }
                () = token.cancelled() => {
                    tracing::info!(
                        "Webhook sender received shutdown signal, finishing pending tasks before shutting down"
                    );

                    shutdown_expected = true;

                    if futures_set.is_empty() {
                        tracing::info!(
                            "No pending tasks, webhook sender is shutting down"
                        );

                        break;
                    }
                }
            }
        }
    }

    pub fn ignite(
        self,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.perform(token).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;
    use kalatori_client::types::{
        InvoiceEventType,
        KalatoriEventExt,
    };
    use kalatori_client::utils::{
        SIGNATURE_HEADER,
        TIMESTAMP_HEADER,
    };
    use mockall::predicate::eq;
    use rust_decimal::Decimal;

    use crate::dao::{
        DaoWebhookEventError,
        MockDaoInterface,
    };
    use crate::types::default_invoice;

    use super::*;

    fn generate_events(count: usize) -> Vec<WebhookEvent> {
        (0..count)
            .map(|_| {
                let mut event: WebhookEvent = default_invoice()
                    .with_amount(Decimal::ZERO)
                    .into_public_invoice("http://shop.example.com")
                    .build_event(InvoiceEventType::Created)
                    .into();

                event.id = Uuid::new_v4();
                event
            })
            .collect()
    }

    #[tokio::test]
    async fn test_send_webhook() {
        let server = MockServer::start();
        let client = reqwest::Client::new();

        // Test case 1:
        // - Successful flow
        // - 200 response
        {
            let ok_mock = server.mock(|when, then| {
                when.method(GET);

                then.status(200);
            });

            let event_id = Uuid::new_v4();

            let expected_result = SendWebhookResult {
                event_id,
                is_ok: true,
            };

            let request = client
                .request(reqwest::Method::GET, server.base_url())
                .build()
                .unwrap();

            let result = send_webhook(client.clone(), request, event_id).await;

            assert_eq!(expected_result, result);
            ok_mock.assert();
        }

        // Test case 2:
        // - Unsuccessful flow
        // - Server responded with non-200
        {
            let non_ok_mock = server.mock(|when, then| {
                when.method(POST);

                then.status(400);
            });

            let event_id = Uuid::new_v4();

            let expected_result = SendWebhookResult {
                event_id,
                is_ok: false,
            };

            let request = client
                .request(reqwest::Method::POST, server.base_url())
                .build()
                .unwrap();

            let result = send_webhook(client.clone(), request, event_id).await;

            assert_eq!(expected_result, result);
            non_ok_mock.assert();
        }

        // Test case 3:
        // - Unsuccessful flow
        // - Invalid server (reqwest error)
        {
            let event_id = Uuid::new_v4();

            let expected_result = SendWebhookResult {
                event_id,
                is_ok: false,
            };

            let request = client
                .request(
                    reqwest::Method::POST,
                    "http://bad.example.com",
                )
                .build()
                .unwrap();

            let result = send_webhook(client.clone(), request, event_id).await;

            assert_eq!(expected_result, result);
        }
    }

    #[tokio::test]
    async fn test_build_request() {
        let dao = MockDaoInterface::default();
        let hmac_config = HmacConfig::new(b"test".to_vec(), 10);

        let sender = WebhookSender::new(
            dao,
            "http://webhook.example.com".to_string(),
            hmac_config,
        );

        let event: WebhookEvent = default_invoice()
            .with_amount(Decimal::ZERO)
            .into_public_invoice("http://shop.example.com")
            .build_event(InvoiceEventType::Created)
            .into();

        let expected_body_string = event.payload.to_string();

        let result = sender.build_request(event);
        assert!(matches!(
            *result.method(),
            reqwest::Method::POST
        ));
        assert_eq!(
            *result.url(),
            "http://webhook.example.com"
                .parse()
                .unwrap()
        );
        assert!(result.timeout().is_some());
        assert_eq!(
            *result.timeout().unwrap(),
            WEBHOOK_SENDER_REQUEST_TIMEOUT
        );
        assert!(result.body().is_some());
        assert_eq!(
            String::from_utf8_lossy(
                result
                    .body()
                    .unwrap()
                    .as_bytes()
                    .unwrap()
            ),
            expected_body_string
        );

        let result_headers = result.headers();
        assert!(result_headers.contains_key(TIMESTAMP_HEADER));
        assert!(result_headers.contains_key(SIGNATURE_HEADER));
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    #[expect(clippy::cast_possible_truncation)]
    async fn test_prepare_webhook_events() {
        let dao = MockDaoInterface::default();
        let hmac_config = HmacConfig::new(b"test".to_vec(), 10);

        let mut sender = WebhookSender::new(
            dao,
            "http://webhook.example.com".to_string(),
            hmac_config,
        );

        // Test case 1:
        // - Empty sender internal queue
        // - Request max limit of events
        // - Return (max - 2) events
        // - Expectations:
        //   - Dao called once
        //   - (max - 2) ids in internal queue
        //   - Ids are equal to ids of returned events
        //   - (max - 2) futures returned
        {
            let returned_events_count = WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS - 2;
            let events = generate_events(returned_events_count);
            let events_ids: HashSet<_> = events.iter().map(|e| e.id).collect();

            sender
                .dao
                .expect_get_webhook_events_to_send()
                .with(eq(
                    WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS as u32,
                ))
                .return_once(|_| Ok(events));

            let result = sender.prepare_webhook_events().await;
            sender.dao.checkpoint();
            assert_eq!(result.len(), returned_events_count);
            assert_eq!(sender.processing_events_ids, events_ids);
        }

        // Test case 2:
        // - Sender internal queue contains (max - 2) ids
        // - Expectations:
        //   - Dao called once
        //   - 2 events requested
        //   - 2 events returned from dao
        //   - Internal queue extended with new ids
        //   - 2 futures returned
        {
            let returned_events_count = 2;
            let events = generate_events(returned_events_count);
            let events_ids: HashSet<_> = events.iter().map(|e| e.id).collect();

            sender
                .dao
                .expect_get_webhook_events_to_send()
                .with(eq(returned_events_count as u32))
                .return_once(|_| Ok(events));

            let result = sender.prepare_webhook_events().await;
            sender.dao.checkpoint();
            assert_eq!(result.len(), returned_events_count);
            assert_eq!(
                sender.processing_events_ids.len(),
                WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS
            );
            assert!(
                sender
                    .processing_events_ids
                    .is_superset(&events_ids)
            );
        }

        // Test case 3:
        // - Sender internal queue contains max ids
        // - Expectations:
        //   - Dao not called
        //   - Empty vector returned
        //   - Internal queue didn't change
        {
            let result = sender.prepare_webhook_events().await;
            assert!(result.is_empty());
            assert_eq!(
                sender.processing_events_ids.len(),
                WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS
            );
        }

        // Test case 4:
        // - Sender internal queue contains 1 predefined id
        // - Expectations:
        //   - Dao is called for (max - 1) limit
        //   - Dao returns 2 events
        //   - One of 2 events returned from dao has id which is already in internal
        //     queue
        //   - Internal queue contains 2 ids
        //   - Function returns 1 future in vec
        {
            // cleanup
            sender.processing_events_ids.clear();
            let returned_events_count = 2;
            let events = generate_events(returned_events_count);
            let events_ids: HashSet<_> = events.iter().map(|e| e.id).collect();
            // insert one of events ids into the internal queue
            sender
                .processing_events_ids
                .insert(events.first().as_ref().unwrap().id);

            sender
                .dao
                .expect_get_webhook_events_to_send()
                .with(eq(
                    (WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS - 1) as u32,
                ))
                .return_once(|_| Ok(events));

            let result = sender.prepare_webhook_events().await;
            sender.dao.checkpoint();
            assert_eq!(result.len(), 1);
            assert_eq!(
                sender.processing_events_ids.len(),
                returned_events_count
            );
            assert_eq!(sender.processing_events_ids, events_ids);
        }

        // Test case 5:
        // - Dao error while trying to fetch events
        // - Expectations
        //   - Dao called once
        //   - Internal queue unchanged
        //   - Error log presented
        //   - Empty vec returned
        {
            let queue = sender.processing_events_ids.clone();
            assert!(!logs_contain(
                "Failed to fetch pending webhook events from database"
            ));

            sender
                .dao
                .expect_get_webhook_events_to_send()
                .with(eq(
                    (WEBHOOK_SENDER_MAX_CONCURRENT_REQUESTS - 2) as u32,
                ))
                .return_once(|_| Err(DaoWebhookEventError::DatabaseError));

            let result = sender.prepare_webhook_events().await;
            sender.dao.checkpoint();
            assert!(result.is_empty());
            assert_eq!(sender.processing_events_ids, queue);
            assert!(logs_contain(
                "Failed to fetch pending webhook events from database"
            ));
        }
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn test_handle_send_webhook_result() {
        let dao = MockDaoInterface::default();
        let hmac_config = HmacConfig::new(b"test".to_vec(), 10);

        let mut sender = WebhookSender::new(
            dao,
            "http://webhook.example.com".to_string(),
            hmac_config,
        );

        let mut events = generate_events(3);
        let (event_1, event_2, event_3) = (
            events.remove(2),
            events.remove(1),
            events.remove(0),
        );
        sender
            .processing_events_ids
            .insert(event_1.id);
        sender
            .processing_events_ids
            .insert(event_2.id);
        sender
            .processing_events_ids
            .insert(event_3.id);

        // Test case 1:
        // - Webhook with ok result
        // - Expectations:
        //   - Webhook id removed from internal queue
        //   - Single DAO call with respective webhook id
        //   - DAO successful result
        {
            let event_id = event_1.id;
            let webhook_result = SendWebhookResult {
                event_id,
                is_ok: true,
            };

            sender
                .dao
                .expect_mark_webhook_event_as_sent()
                .with(eq(event_id))
                .return_once(move |_| Ok(event_1));

            sender
                .handle_send_webhook_result(webhook_result)
                .await;
            sender.dao.checkpoint();
            assert_eq!(sender.processing_events_ids.len(), 2);
            assert!(
                !sender
                    .processing_events_ids
                    .contains(&event_id)
            );
        }

        // Test case 2:
        // - Webhook with not ok result
        // - Expectations:
        //   - Webhook id removed from internal queue
        //   - No DAO calls
        {
            let event_id = event_2.id;
            let webhook_result = SendWebhookResult {
                event_id,
                is_ok: false,
            };

            sender
                .handle_send_webhook_result(webhook_result)
                .await;
            sender.dao.checkpoint();
            assert_eq!(sender.processing_events_ids.len(), 1);
            assert!(
                !sender
                    .processing_events_ids
                    .contains(&event_id)
            );
        }

        // Test case 3:
        // - Error while mark webhook as sent
        // - Expectations:
        //   - Webhook id removed from internal queue
        //   - Single DAO call
        //   - Error log recorded
        {
            assert!(!logs_contain(
                "Failed to mark webhook event as sent in database. It might be resent"
            ));
            let event_id = event_3.id;
            let webhook_result = SendWebhookResult {
                event_id,
                is_ok: true,
            };

            sender
                .dao
                .expect_mark_webhook_event_as_sent()
                .with(eq(event_id))
                .return_once(move |_| Err(DaoWebhookEventError::DatabaseError));

            sender
                .handle_send_webhook_result(webhook_result)
                .await;
            sender.dao.checkpoint();
            assert!(sender.processing_events_ids.is_empty());
            assert!(logs_contain(
                "Failed to mark webhook event as sent in database. It might be resent"
            ));
        }
    }
}
