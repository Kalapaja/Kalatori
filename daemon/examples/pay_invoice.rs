use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::{Address, address, keccak256, U256};
use alloy::providers::ProviderBuilder;
use alloy::providers::ext::AnvilApi;
use alloy::sol;
use alloy::sol_types::SolValue;
use rust_decimal::Decimal;
use uuid::Uuid;

use kalatori_client::KalatoriClient;
use kalatori_client::types::{CreateInvoiceParams, Invoice, InvoiceCart};

sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    contract IERC20 {
        function balanceOf(address target) returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
    }
);

// Anvil test account (4)
static INVOICE_PAYER: Address = address!("0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65");
static USDC_ADDRESS: Address = address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359");
static USDC_STORAGE_SLOT: u8 = 9;

async fn create_invoice() -> Invoice {
    let client = KalatoriClient::new(
        "http://localhost:8080".to_string(),
        "secret".as_bytes().to_vec(),
    );

    // Create an invoice
    let create_params = CreateInvoiceParams {
        order_id: Uuid::new_v4().to_string(),
        amount: Decimal::ONE_HUNDRED,
        cart: InvoiceCart::empty(),
        redirect_url: "http://example.com/redirect".to_string(),
        include_transactions: false,
    };

    client
        .create_invoice(create_params)
        .await
        .expect("Failed to create invoice")
        .expect("Failed to create invoice")
}

// Before running the example make sure that anvil and alto containers are started
#[tokio::main]
async fn main() {
    // Connect to local Anvil node
    let rpc_url = "http://localhost:8545".parse().expect("Invalid url");
    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let usdc_contract = IERC20::new(USDC_ADDRESS, provider.clone());

    let invoice = create_invoice().await;
    let invoice_wallet = Address::from_str(&invoice.payment_address).expect("Failed to parse payment address");

    // Add 1000 USDC to the payer account. By default anvil test accounts
    // dont have any USDC, so we have to reset storage
    let hashed_slot = keccak256((INVOICE_PAYER, U256::from(USDC_STORAGE_SLOT)).abi_encode());
    let mocked_balance = U256::from(1_000_000_000);

    provider.anvil_set_storage_at(
        USDC_ADDRESS,
        hashed_slot.into(),
        mocked_balance.into()
    )
    .await
    .expect("Failed to set storage at 1000 USDC");

    // Check that USDC is on invoice payer's balance
    let invoice_payer_balance = usdc_contract
        .balanceOf(INVOICE_PAYER)
        .call()
        .await
        .expect("Failed to get account balance");

    assert_eq!(invoice_payer_balance, mocked_balance);

    // Send 100 USDC to invoice wallet
    let _tx = usdc_contract
        .transfer(invoice_wallet, U256::from(100_000_000))
        .from(INVOICE_PAYER)
        .send()
        .await
        .expect("Failed to send transaction");

    let new_invoice_payer_balance = usdc_contract
        .balanceOf(INVOICE_PAYER)
        .call()
        .await
        .expect("Failed to get account balance");

    assert_eq!(new_invoice_payer_balance, U256::from(900_000_000));

    // Check that invoice was fully paid and has 100 USDC on balance
    let invoice_wallet_balance = usdc_contract
        .balanceOf(invoice_wallet)
        .call()
        .await
        .expect("Failed to get account balance");

    assert_eq!(invoice_wallet_balance, U256::from(100_000_000));

    // Waiting withdrawal to proceed.
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Check invoice wallet balance again. It may still have few cents after withdrawal.
    // Gas prices very, so let's check less than 5 USDC remains for instance.
    let invoice_wallet_balance = usdc_contract
        .balanceOf(invoice_wallet)
        .call()
        .await
        .expect("Failed to get account balance");

    assert!(invoice_wallet_balance < U256::from(5_000_000));
    println!("Invoice wallet balance after withdrawal: {}", invoice_wallet_balance);
}
