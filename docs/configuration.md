# Configuration

Kalatori is configured through JSON files and/or environment variables. Environment variables take precedence over file values.

## Quick Start

```bash
# Copy example configs
make copy-configs
```

### For Development / Testing

The example configs work out of the box for local development and API testing without real transfers. Just copy them and run the daemon:

```bash
make setup
make run
```

!!! note
    The payment page (front-end) requires a valid Reown project ID to work correctly. Without it, wallet connection will fail. Get one for free at [cloud.reown.com](https://cloud.reown.com/) and set it in `shop.json` → `reown_project_id`.

### For Real Transfers

When handling real money, you **must** configure the following:

| What | Where | How to Get |
|------|-------|------------|
| Seed phrase | `secrets.json` → `seed` | Generate a unique BIP39 mnemonic — the example seed is publicly known and must not be used for real funds |
| Recipient address | `payments.json` → `recipient` | Your own wallet address for used chain (Polygon by default) |
| Reown project ID | `shop.json` → `reown_project_id` | Register at [cloud.reown.com](https://cloud.reown.com/) (free) |
| Etherscan API key | `etherscan_client.json` → `api_key` | Register at [polygonscan.com/apis](https://polygonscan.com/apis) (free, required for Polygon) |
| Webhook URL | `shop.json` → `invoices_webhook_url` | Your e-commerce platform's endpoint, or use [webhook.site](https://webhook.site) for testing |

### For Production

On top of the above, for a production deployment you should also:

- **Generate a new API secret key** (`secrets.json` → `api_secret_key`) — use a strong random value, e.g.: `openssl rand -base64 32`
- **Set your public URL** (`payments.json` → `payment_url_base`) — the externally accessible URL of your Kalatori instance
- **Set your shop name and logo** (`shop.json` → `shop_name`, `logo_url`) — displayed on the payment page

## Config Files

All files are loaded from the `configs/` directory (override with `KALATORI_CONFIG_DIR_PATH` env var). Every file is optional on the filesystem — you can provide all values via environment variables instead.

### secrets.json

Sensitive credentials. Both fields are required.

```json
{
  "seed": "your twelve or twenty four word mnemonic phrase here",
  "api_secret_key": "your-api-secret-key"
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `seed` | Yes | BIP39 mnemonic phrase (12 or 24 words). Used to deterministically derive unique payment accounts for each invoice. The same seed always produces the same accounts, so it's safe to restart the daemon without losing track of payments. |
| `api_secret_key` | Yes | Secret key for webhook HMAC signature verification. Must match the key configured in your e-commerce platform. |

**Where to get values:**

- **seed**: Generate a BIP39 mnemonic using any trusted tool. For production, use an offline generator or a hardware wallet. Never reuse a seed phrase that holds personal funds.
- **api_secret_key**: Generate a random string, e.g.: `openssl rand -base64 32`

!!! warning "Security"
    Both values are automatically removed from environment variables after loading to prevent accidental exposure. They are stored in memory using `SecretString` which zeroes memory on drop.

### payments.json

Payment routing and invoice settings.

```json
{
  "recipient": {
    "PolkadotAssetHub": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
    "Polygon": "0x0E3Ca7fD040144900AdaA5f9B8917f3933A4F5e9"
  },
  "payment_url_base": "https://pay.example.com"
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `recipient` | Yes | — | Wallet addresses where paid invoices are swept to, one per chain. At minimum, the address for the default chain must be set. |
| `payment_url_base` | Yes | — | Public URL of your Kalatori instance. Used to generate payment page links. |
| `invoice_lifetime_millis` | No | `86400000` (24h) | How long an invoice stays active before expiring, in milliseconds. |
| `default_chain` | No | `Polygon` | Which chain to use when creating invoices without specifying a chain. |
| `default_asset_id` | No | Per-chain built-in | Default asset for each chain. PolkadotAssetHub: `1337`, Polygon: `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` (native USDC). |
| `slippage_params` | No | All zeros | Per-asset underpay/overpay tolerance. See [Slippage Parameters](#slippage-parameters). |

**Where to get values:**

- **recipient (Polygon)**: Your Ethereum/Polygon wallet address in 0x format. Get from MetaMask, Ledger, etc.
- **recipient (PolkadotAssetHub)**: Your Polkadot wallet address in ss58 format (prefix 0). Get from Polkadot.js, Nova Wallet, Ledger, etc. Note: the front-end payment page does not support Polkadot Asset Hub yet.

### shop.json

Shop metadata and webhook integration for the payment page.

```json
{
  "invoices_webhook_url": "https://mystore.com/webhooks/kalatori",
  "shop_name": "My Store",
  "logo_url": "https://mystore.com/logo.png",
  "reown_project_id": "da9b8666eec49849ccb28bca96afdefa"
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `invoices_webhook_url` | Yes | — | URL where Kalatori sends invoice status updates (paid, expired, etc.). |
| `shop_name` | Yes | — | Display name shown on the payment page. |
| `reown_project_id` | Yes | — | Reown (formerly WalletConnect) project ID for wallet connection on the payment page. |
| `logo_url` | No | `null` | URL to shop logo image displayed on the payment page. |
| `signature_max_age_secs` | No | `300` (5 min) | Maximum age of webhook HMAC signature before rejection. Prevents replay attacks. |

**Where to get values:**

- **reown_project_id**: Register at [cloud.reown.com](https://cloud.reown.com/), create a project, and copy the Project ID. Free tier is available.
- **invoices_webhook_url**: Your e-commerce platform's endpoint that will receive invoice status notifications.

### chains.json

Blockchain RPC endpoints and monitored assets. Fully optional — defaults are provided for all chains.

```json
{
  "chains": {
    "PolkadotAssetHub": {
      "endpoints": ["wss://asset-hub-polkadot-rpc.n.dwellir.com"],
      "allow_insecure_endpoints": false
    },
    "Polygon": {
      "endpoints": ["wss://polygon-bor-rpc.publicnode.com"],
      "allow_insecure_endpoints": false
    }
  }
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `chains.[ChainType].endpoints` | No | Built-in public RPCs | WebSocket RPC endpoints. Multiple endpoints enable automatic failover. |
| `chains.[ChainType].assets` | No | Default USDC per chain | Asset IDs to monitor. Cannot be overridden via env vars — only via JSON. |
| `chains.[ChainType].allow_insecure_endpoints` | No | `false` | Allow `ws://` and `http://` endpoints. Set to `true` only for local development. |

Default endpoints:

- **PolkadotAssetHub**: `wss://asset-hub-polkadot-rpc.n.dwellir.com`, `wss://polkadot-asset-hub-rpc.polkadot.io`
- **Polygon**: `wss://polygon-bor-rpc.publicnode.com`, `wss://polygon.drpc.org`

### etherscan_client.json

Required for Polygon chain support. Not needed if you only use Polkadot Asset Hub.

```json
{
  "api_key": "YOUR_POLYGONSCAN_API_KEY"
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `api_key` | Yes (for Polygon) | — | API key for Polygonscan, used to track token transfers. |
| `requests_per_second` | No | `3` | Rate limit for API calls. Free tier allows 3 req/s. |

**Where to get values:**

- **api_key**: Register at [polygonscan.com/apis](https://polygonscan.com/apis), create an API key in your account settings. Free tier is sufficient.

### web_server.json

HTTP server binding. Fully optional.

```json
{
  "host": "0.0.0.0",
  "port": 8080
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `host` | No | `0.0.0.0` | IP address to bind to. Use `127.0.0.1` to restrict to localhost. |
| `port` | No | `8080` | TCP port for the HTTP server. |

### database.json

SQLite database settings. Fully optional.

```json
{
  "dir": "database",
  "temporary": false
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `dir` | No | `./database` | Directory where `kalatori_db.sqlite` is stored. |
| `temporary` | No | `false` | Use in-memory database (data lost on shutdown). Useful for testing only. |

### logger.json

Logging configuration. Fully optional.

```json
{
  "directives": "kalatori=trace,info",
  "loki_url": null
}
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `directives` | No | `kalatori=trace,info` | [Tracing filter directives](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) controlling log verbosity. |
| `loki_url` | No | `null` | Grafana Loki URL for centralized log aggregation. If `null`, logs go to stdout only. |

## Environment Variables

Flat config fields can be set via environment variables. Fields with nested structure (like `recipient`, `chains`, `slippage_params`, `assets`) should be configured via JSON files. The pattern is:

```
{PREFIX}_{CONFIG}_{FIELD}
```

where `PREFIX` defaults to `KALATORI`.

### Examples

```bash
# Secrets
export KALATORI_SECRETS_SEED="your mnemonic phrase here"
export KALATORI_SECRETS_API_SECRET_KEY="your-secret-key"

# Payments (flat fields only — recipient and slippage_params should be set via JSON)
export KALATORI_PAYMENTS_PAYMENT_URL_BASE="https://pay.example.com"
export KALATORI_PAYMENTS_INVOICE_LIFETIME_MILLIS=86400000
export KALATORI_PAYMENTS_DEFAULT_CHAIN=Polygon

# Shop
export KALATORI_SHOP_SHOP_NAME="My Store"
export KALATORI_SHOP_INVOICES_WEBHOOK_URL="https://mystore.com/webhooks"
export KALATORI_SHOP_REOWN_PROJECT_ID="da9b8666eec49849ccb28bca96afdefa"

# Web server
export KALATORI_WEB_SERVER_HOST=0.0.0.0
export KALATORI_WEB_SERVER_PORT=8080

# Database
export KALATORI_DATABASE_DIR=/var/lib/kalatori

# Etherscan
export KALATORI_ETHERSCAN_CLIENT_API_KEY="your-api-key"

# Logger
export KALATORI_LOGGER_DIRECTIVES="kalatori=debug,info"
export KALATORI_LOGGER_LOKI_URL="http://localhost:3100"
```

!!! note
    Fields with nested structure (`recipient`, `slippage_params`, `chains`) should be configured via JSON files rather than environment variables.

### Special Variables

| Variable | Description |
|----------|-------------|
| `KALATORI_APP_ENV_PREFIX` | Change the prefix from `KALATORI` to a custom value. All other env vars must then use the new prefix. |
| `KALATORI_CONFIG_DIR_PATH` | Override the config files directory (default: `configs`). |

### Priority

Environment variables override JSON file values. Built-in defaults apply when neither is set.

```
Environment variable > JSON file > Built-in default
```

## Slippage Parameters

Slippage parameters control how the daemon handles payments that don't exactly match the invoice amount. Configured per asset in `payments.json`:

```json
{
  "slippage_params": {
    "Polygon": {
      "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359": {
        "underpayment_tolerance": "0.05",
        "overpayment_tolerance": "0.10"
      }
    }
  }
}
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `underpayment_tolerance` | `0` | Maximum amount below the invoice that is still accepted as paid. `0` means exact amount required. |
| `overpayment_tolerance` | `0` | Maximum excess above the invoice before triggering a partial refund. `0` means any overpayment triggers refund. |

## Supported Chains

| Chain | Address Format | Default Asset |
|-------|---------------|---------------|
| `PolkadotAssetHub` | ss58 (prefix 0), e.g. `5Grwva...` | `1337` (USDC) |
| `Polygon` | 0x hex (ERC-20), e.g. `0x0E3C...` | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` (native USDC) |
