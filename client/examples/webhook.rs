use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{
    Json,
    Router,
    serve,
};
use rust_decimal::Decimal;
use uuid::Uuid;

use kalatori_client::KalatoriClient;
use kalatori_client::middleware::axum_hmac_validator;
use kalatori_client::types::{
    CreateInvoiceParams,
    GenericEvent,
    Invoice,
    InvoiceCart,
};
use kalatori_client::utils::HmacConfig;

async fn webhook_listener(Json(event): Json<GenericEvent<Invoice>>) -> impl IntoResponse {
    println!("Received event: {:#?}", event);
    (
        axum::http::StatusCode::OK,
        "Event received",
    )
}

#[tokio::main]
async fn main() {
    let hmac_config = HmacConfig::new("secret".as_bytes().to_vec(), 60);

    let client = KalatoriClient::new(
        "http://localhost:8080".to_string(),
        "secret".as_bytes().to_vec(),
    );

    let app = Router::new()
        .route(
            "/webhooks/invoices",
            post(webhook_listener),
        )
        .layer(from_fn_with_state(
            hmac_config,
            axum_hmac_validator,
        ));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .unwrap();

    // Start the server in background
    tokio::spawn(async move {
        serve(listener, app).await.unwrap();
    });

    // Create an invoice to trigger the webhook
    let payload = CreateInvoiceParams {
        order_id: Uuid::new_v4().to_string(),
        amount: Decimal::new(5, 1), // 0.10
        cart: InvoiceCart::empty(),
        redirect_url: "http://example.com/thank-you".to_string(),
        include_transactions: false,
    };

    let invoice = client
        .create_invoice(payload)
        .await
        .unwrap()
        .unwrap();

    println!("Created invoice: {:#?}", invoice);

    // Keep the main task alive to receive the webhook
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
}
