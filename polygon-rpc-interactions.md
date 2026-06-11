# Exhaustive Polygon RPC Interaction Catalog

Every external network call the Kalatori daemon makes when settling payments on Polygon, organized by subsystem, lifecycle phase, and protocol endpoint.

---

## 1. Polygon RPC Node (Alloy Provider over WebSocket)

Connection: `ProviderBuilder::new().connect_ws(...)` to a randomly-selected endpoint from `config.endpoints`.

### 1.1 Initialization

| # | Underlying JSON-RPC | Alloy API | Purpose | File:Line |
|---|---------------------|-----------|---------|-----------|
| 1 | `eth_chainId` | `provider.get_chain_id()` | Verify we connected to chain 137 (Polygon Mainnet) | `polygon.rs:308-318` |
| 2 | `eth_call` → `symbol()` | `IERC20::new(asset, provider).symbol().call()` | Fetch ERC-20 token symbol (e.g. "USDC") | `polygon.rs:614-627` |
| 3 | `eth_call` → `decimals()` | `IERC20::new(asset, provider).decimals().call()` | Fetch ERC-20 token decimal places (6 for USDC) | `polygon.rs:630-643` |

Called once per configured asset at startup via `init_asset_info`.

### 1.2 Payment Monitoring

| # | Underlying JSON-RPC | Alloy API | Purpose | File:Line |
|---|---------------------|-----------|---------|-----------|
| 4 | `eth_subscribe("logs", {filter})` | `provider.subscribe_logs(&filter)` | Open a persistent WebSocket subscription for ERC-20 `Transfer(address,address,uint256)` events across all tracked token contracts | `polygon.rs:727-739` |

- Filter: address = configured ERC-20 contracts, topic0 = `Transfer` event signature hash (`0xddf252ad...`)
- Runs as a `try_stream!` in `TransfersTracker`; each log decoded as `IERC20::Transfer`
- On stream failure: calls `client.recreate()` which picks a new endpoint from the config and re-subscribes (`transfer_tracker.rs:169-195`)

### 1.3 Transaction Building (`build_transfer` / `build_transfer_all`)

| # | Underlying JSON-RPC | Alloy API | Purpose | File:Line |
|---|---------------------|-----------|---------|-----------|
| 5 | `eth_getTransactionCount` | `provider.get_transaction_count(sender)` | Get sender EOA nonce (for EIP-7702 authorization) | `polygon.rs:818-830` |
| 6 | `eth_call` → `nonces(address)` | `IERC20::new(USDC, provider).nonces(sender).call()` | Get ERC-2612 permit nonce (prevents replay) | `polygon.rs:832-844` |
| 7 | `eth_call` → `getNonce(address, uint192)` | `IERC20::new(ENTRYPOINT, provider).getNonce(sender, 0).call()` | Get ERC-4337 EntryPoint nonce for UserOperation | `polygon.rs:846-861` |
| 8 | `eth_call` → `balanceOf(address)` | `IERC20::new(asset, provider).balanceOf(sender).call()` | Query current token balance (for `transfer_all`: determines gross amount) | `polygon.rs:678-692` |

Call #8 is made in `build_transfer_all` (before calling `build_transfer`), and also during expiration checking.

### 1.4 Expiration Checking

| # | Underlying JSON-RPC | Alloy API | Purpose | File:Line |
|---|---------------------|-----------|---------|-----------|
| 9 | `eth_call` → `balanceOf(address)` | `polygon_client.fetch_asset_balance(...)` | Verify on-chain balance against recorded received amount before marking invoice expired | `expiration_detector.rs:117-124` |

Called every 10 seconds for each expired invoice, via `ExpirationDetector`.

---

## 2. Pimlico Bundler (HTTP JSON-RPC)

Endpoint: `https://public.pimlico.io/v2/137/rpc` (hardcoded in `consts.rs:7`).
Transport: `reqwest::Client` POST with hand-rolled JSON-RPC 2.0 envelope (`pimlico_client.rs:246-266`).

### 2.1 Transaction Signing Phase (`sign_transaction`)

| # | JSON-RPC Method | Purpose | File:Line |
|---|-----------------|---------|-----------|
| 10 | `pimlico_getUserOperationGasPrice` | Get `slow`/`standard`/`fast` gas price recommendations (`maxFeePerGas`, `maxPriorityFeePerGas`). Currently uses `standard`. | `pimlico_client.rs:268-274`, called at `polygon.rs:863-877` |
| 11 | `eth_estimateUserOperationGas` | Estimate 5 gas components for the UserOperation (with dummy signature). Enforces `paymaster_post_op_gas_limit >= 15000` post-call to avoid AA23 errors. | `pimlico_client.rs:276-301`, called at `polygon.rs:995-1014` |
| 12 | `pimlico_getTokenQuotes` | Get USDC-to-native exchange rate from Circle paymaster for fee calculation. **Only for `transfer_all` operations** — used to compute `amount = balance - max_gas_cost_in_USDC - 100 wei buffer`. | `pimlico_client.rs:325-340`, called at `polygon.rs:1028-1041` |

### 2.2 Transaction Submission & Monitoring (`submit_and_watch_transaction`)

| # | JSON-RPC Method | Purpose | File:Line |
|---|-----------------|---------|-----------|
| 13 | `eth_sendUserOperation` | Submit signed PackedUserOperation to the bundler for inclusion. Returns an operation hash. | `pimlico_client.rs:303-312`, called at `polygon.rs:1124-1137` |
| 14 | `eth_getUserOperationReceipt` | Poll for execution result. Called up to **30 times** at 1-second intervals. Returns `UserOperationReceiptResult` with `success` flag, on-chain `transactionHash`, gas usage. | `pimlico_client.rs:314-323`, called at `polygon.rs:1140-1175` |

---

## 3. Etherscan / Polygonscan API (HTTP REST)

Endpoint: `https://api.etherscan.io/v2/api` with `chain_id=137`.
Transport: `reqwest::Client` GET, rate-limited by `governor` crate (`etherscan_client.rs:77`).

| # | API Call | Purpose | File:Line |
|---|---------|---------|-----------|
| 15 | `module=account&action=tokentx&chain_id=137&contract_address=...&address=...` | Fetch historical ERC-20 token transfers for an account. Used **only during invoice expiration** to reconcile missing incoming transactions when on-chain balance > recorded received amount. Results filtered to incoming transfers (`to == account`). 60s timeout. | `etherscan_client.rs:71-129`, called at `expiration_detector.rs:149-160` |

---

## Summary: Complete Call Inventory

| # | Target | Protocol | Method | Direction | Trigger | Frequency |
|---|--------|----------|--------|-----------|---------|-----------|
| 1 | Polygon RPC | WS | `eth_chainId` | Read | Client init | Once per connection |
| 2 | Polygon RPC | WS | `eth_call` (symbol) | Read | Asset init | Once per asset |
| 3 | Polygon RPC | WS | `eth_call` (decimals) | Read | Asset init | Once per asset |
| 4 | Polygon RPC | WS | `eth_subscribe` (logs) | Subscription | Tracker start | Persistent stream; re-established on failure |
| 5 | Polygon RPC | WS | `eth_getTransactionCount` | Read | Payout build | Once per payout |
| 6 | Polygon RPC | WS | `eth_call` (USDC nonces) | Read | Payout build | Once per payout |
| 7 | Polygon RPC | WS | `eth_call` (EntryPoint getNonce) | Read | Payout build | Once per payout |
| 8 | Polygon RPC | WS | `eth_call` (balanceOf) | Read | Payout build (transfer_all) | Once per payout |
| 9 | Polygon RPC | WS | `eth_call` (balanceOf) | Read | Expiration check | Per expired invoice, every 10s |
| 10 | Pimlico | HTTP | `pimlico_getUserOperationGasPrice` | Read | Payout build | Once per payout |
| 11 | Pimlico | HTTP | `eth_estimateUserOperationGas` | Read | Payout sign | Once per payout |
| 12 | Pimlico | HTTP | `pimlico_getTokenQuotes` | Read | Payout sign (transfer_all only) | Once per transfer_all payout |
| 13 | Pimlico | HTTP | `eth_sendUserOperation` | Write | Payout submit | Once per payout |
| 14 | Pimlico | HTTP | `eth_getUserOperationReceipt` | Read | Payout monitor | Up to 30x per payout (1s interval) |
| 15 | Etherscan | HTTP | `tokentx` REST API | Read | Expiration reconciliation | Per expired Polygon invoice with balance mismatch |

---

## Architecture Notes

### No direct `eth_sendRawTransaction`
All outbound transfers go through the Pimlico ERC-4337 bundler. The daemon never broadcasts raw transactions to the Polygon RPC node.

### Three nonces per payout
Each payout requires fetching three separate nonces:
- **EOA nonce** (`eth_getTransactionCount`) — for the EIP-7702 authorization that delegates the EOA to the smart account implementation
- **Permit nonce** (USDC `nonces()`) — for the EIP-2612 permit signature that approves the Circle paymaster to deduct fees in USDC
- **EntryPoint nonce** (`getNonce()`) — for the ERC-4337 UserOperation sequence number

### Gas fee model
The Circle paymaster (`0x0578...`) covers gas in USDC. For `transfer_all`, the daemon queries `pimlico_getTokenQuotes` to get the USDC/POL exchange rate, computes `total_gas * maxFeePerGas * exchange_rate / 10^18`, and subtracts that plus a 100-wei buffer from the transfer amount.

### Failover
- **Polygon RPC**: random endpoint selection from configured list; on subscription failure, `recreate()` picks a new endpoint
- **Pimlico**: no failover (single hardcoded public endpoint)
- **Etherscan**: no failover (single endpoint), rate-limited

### Key contracts

| Name | Address | Used For |
|------|---------|----------|
| USDC (native) | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` | Token transfers, balance queries, permit nonces |
| ERC-4337 EntryPoint | `0x4337084D9E255Ff0702461CF8895CE9E3b5Ff108` | UserOperation nonce, bundler target |
| Circle Paymaster | `0x0578cFB241215b77442a541325d6A4E6dFE700Ec` | Gas sponsorship via USDC permit |
| Account Implementation | `0xe6Cae83BdE06E4c305530e199D7217f42808555B` | EIP-7702 delegation target (smart account logic) |
