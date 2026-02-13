use alloy::primitives::{Address, address, keccak256, U256};
use alloy::providers::ProviderBuilder;
use alloy::providers::ext::AnvilApi;
use alloy::sol;
use alloy::sol_types::SolValue;

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

#[tokio::main]
async fn main() {
    let rpc_url = "http://localhost:8545".parse().expect("Invalid url");
    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let usdc_contract = IERC20::new(USDC_ADDRESS, provider.clone());

    // Paste your invoice wallet here
    let invoice_wallet = address!("0x3A0b9D2667A8b8AB6765339Cf32e6c7699eb5509");

    // Add 1000 USDC to the payer account
    let hashed_slot = keccak256((INVOICE_PAYER, U256::from(USDC_STORAGE_SLOT)).abi_encode());
    let mocked_balance = U256::from(1_000_000_000);

    provider.anvil_set_storage_at(
        USDC_ADDRESS,
        hashed_slot.into(),
        mocked_balance.into()
    )
    .await
    .expect("Failed to set storage at 1000 USDC");

    // Check USDC balance
    let current_balance = usdc_contract
        .balanceOf(INVOICE_PAYER)
        .call()
        .await
        .expect("Failed to get account balance");

    println!("USDC balance after airdrop: {}", current_balance);

    // Send 100 USDC to invoice wallet
    let _tx = usdc_contract
        .transfer(invoice_wallet, U256::from(100_000_000))
        .from(INVOICE_PAYER)
        .send()
        .await
        .expect("Failed to send transaction");

    let current_balance = usdc_contract
        .balanceOf(INVOICE_PAYER)
        .call()
        .await
        .expect("Failed to get account balance");

    println!("USDC balance after sending money: {}", current_balance);

    let invoice_wallet = usdc_contract
        .balanceOf(invoice_wallet)
        .call()
        .await
        .expect("Failed to get account balance");

    println!("INVOICE WALLET BALANCE AFTER PAY: {}", invoice_wallet);
}
