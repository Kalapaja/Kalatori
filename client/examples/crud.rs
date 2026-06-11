use rust_decimal::Decimal;
use uuid::Uuid;

use kalatori_client::KalatoriClient;
use kalatori_client::types::{
    CreateInvoiceParams,
    GetInvoiceParams,
    InvoiceCart,
    InvoiceCartItem,
    UpdateInvoiceParams,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = KalatoriClient::new(
        "http://localhost:8080".to_string(),
        "secret".as_bytes().to_vec(),
    );

    // Create an invoice
    let create_params = CreateInvoiceParams {
        order_id: Uuid::new_v4().to_string(),
        amount: Decimal::ONE_HUNDRED,
        cart: InvoiceCart::empty(),
        metadata: Some(serde_json::json!({"external_ref": "example-42"})),
        redirect_url: "http://example.com/redirect".to_string(),
        include_transactions: false,
    };

    let created_invoice = client
        .create_invoice(create_params)
        .await??;
    println!(
        "Created Invoice: {:#?}",
        created_invoice
    );

    // Get the invoice
    let get_params = GetInvoiceParams {
        invoice_id: created_invoice.id,
        include_transactions: false,
    };

    let fetched_invoice = client.get_invoice(get_params).await??;
    println!(
        "Fetched Invoice: {:#?}",
        fetched_invoice
    );

    assert_eq!(created_invoice, fetched_invoice);

    let cart = InvoiceCart {
        items: vec![InvoiceCartItem {
            name: "Updated Item 1".to_string(),
            quantity: 100,
            price: Decimal::TEN,
            product_url: None,
            image_url: None,
            tax: None,
            discount: None,
        }],
    };

    // Update the invoice
    let update_params = UpdateInvoiceParams {
        invoice_id: created_invoice.id,
        amount: Decimal::ONE_THOUSAND,
        cart,
        metadata: created_invoice.metadata.clone(),
        include_transactions: false,
    };

    let updated_invoice = client
        .update_invoice(update_params)
        .await??;
    println!(
        "Updated Invoice: {:#?}",
        updated_invoice
    );

    assert_ne!(created_invoice, updated_invoice);
    assert!(!updated_invoice.cart.is_empty());

    // Cancel the invoice
    let cancel_params = GetInvoiceParams {
        invoice_id: created_invoice.id,
        include_transactions: false,
    };

    let canceled_invoice = client
        .cancel_invoice(cancel_params)
        .await??;
    println!(
        "Canceled Invoice: {:#?}",
        canceled_invoice
    );

    assert!(canceled_invoice.status.is_canceled());

    // Get unexisting invoice, expect error
    let get_params = GetInvoiceParams {
        invoice_id: Uuid::new_v4(),
        include_transactions: false,
    };

    let result = client
        .get_invoice(get_params)
        .await?
        .unwrap_err();
    println!(
        "Expected error fetching non-existing invoice: {:#?}",
        result
    );

    Ok(())
}
