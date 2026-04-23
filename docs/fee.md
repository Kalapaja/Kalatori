# Commission System

## Overview

Kalatori can charge a platform fee on outgoing merchant payouts. The fee is collected atomically in the same on-chain transaction as the payout — no separate settlement, no custody of funds.

Commission is opt-in: if no commission config is present, no fee is charged. The rate and destination wallet come either from a per-client decision service or from static config values.

---

## Product Behavior

When commission applies, Kalatori splits the payout into two simultaneous transfers:

```
Invoice: 100 USDC received
├── Merchant:    99.60 USDC  →  merchant's destination address
└── Fee wallet:   0.40 USDC  →  fee address
```

The merchant receives `amount − gas_fees − commission`. The customer always pays exactly the invoice amount.

Commission applies to **outgoing payouts only**. Refunds are not charged.

---

## Money Flow

Three parties participate in every payment:

- **Customer** — the end user making a purchase
- **Merchant** — the business receiving payment, operates or contracts a Kalatori daemon
- **Daemon provider** — the entity running the Kalatori daemon (may be the merchant themselves in self-hosted deployments, or a SaaS operator)

### Flow

```
Customer
   │
   │  pays invoice amount (e.g. 100 USDC)
   │  to unique invoice address (HD-derived, controlled by daemon)
   ▼
Invoice address (Kalatori-managed EOA)
   │
   │  daemon detects payment, queues payout
   │  deducts gas fees (paid in USDC via Circle paymaster)
   │
   ├──── 99.6 USDC ────────────────────────────► Merchant
   │                                              destination address
   │
   └──── 0.4 USDC (commission) ────────────────► Daemon provider
                                                  fee_wallet
```

### How the daemon provider participates

The daemon provider earns commission on every processed payout. They configure the `fee_wallet` address (where commission is collected) and, optionally, a decision service that can vary the rate or waive the fee per client.

In a self-hosted deployment (merchant runs their own daemon), both roles belong to the same entity — the merchant effectively pays no commission to a third party unless they configure one.

In a managed deployment (SaaS provider runs the daemon on behalf of merchants), the provider sets the fee in `commission.json` and earns the commission as service revenue. The merchant sees a reduced payout net amount; the customer is unaffected.

The daemon provider also absorbs the UserOperation gas cost, which is deducted from the payout before the merchant/commission split. The provider bears no gas exposure — gas is always recovered from the payout amount.

---

## Architecture

### Payment lifecycle

```
Customer pays invoice
        │
        ▼
Incoming transfer detected & recorded
        │
        ▼
Payout queued (status: Waiting)
        │
        ▼
┌───────────────────────────────┐
│  Commission decision          │
│  (service call or config)     │
│  → fee_wallet, fee_bps        │
└───────────────────────────────┘
        │
   ┌────┴──────────┐
fee_bps > 0     fee_bps = 0
   │                │
   ▼                ▼
executeBatch     execute
(2 transfers)   (1 transfer)
        │
        ▼
UserOperation submitted → confirmed on-chain
        │
        ▼
Payout marked Completed, webhook fired
```

### Commission decision service

An optional microservice called once per payout before building the transaction. The decision is per client — the service can apply different rates, exemptions, or trial periods per merchant.

**Request** (POST to `commission_service_url`):
```json
{
  "client_id": "string",
  "chain": "Polygon",
  "amount": "100.000000"
}
```

**Response — apply commission**:
```json
{
  "apply": true,
  "fee_wallet": "0x...",
  "fee_bps": 40
}
```

**Response — skip commission**:
```json
{
  "apply": false
}
```

`fee_wallet` and `fee_bps` are required when `apply` is `true`. The service can return different wallets per client (e.g. per-reseller routing).

**Hard limit**: `fee_bps` is capped to **100 (1%)** regardless of what the service returns. Values above the limit are silently capped and a warning is logged.

### On-chain execution (Polygon)

Kalatori uses [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) to delegate payment accounts to a `Simple7702Account` implementation, enabling batch calls within a single [EIP-4337](https://eips.ethereum.org/EIPS/eip-4337) UserOperation via the Pimlico bundler.

**No commission** — single transfer:
```
execute(dest=USDC, value=0, data=transfer(merchant, amount))
```

**With commission** — atomic batch:
```
executeBatch([
  Call { target: USDC, value: 0, data: transfer(merchant, amount − fee) },
  Call { target: USDC, value: 0, data: transfer(fee_wallet, fee) },
])
```

Gas is deducted from the payout amount before commission is calculated. Gas is paid in USDC via the Circle paymaster — no POL required in the payment account.

### Configuration

Configured via `commission.json` or environment variables:

| Field | Default | Description |
|---|---|---|
| `fee_wallet` | — | Fallback address that receives commission transfers |
| `fee_bps` | `0` | Fallback fee rate in basis points (`0` = no commission) |
| `commission_service_url` | — | Optional URL of the commission decision service |

> `client_id` is not configured here — it is taken automatically from `auth.json` (`OAuthConfig.client_id`) when auth is enabled.

**Resolution order** per payout:

1. **Service present**: call the service. On success, use the `fee_wallet` and `fee_bps` it returns.
2. **Fallback**: on service failure or if no URL is configured, use `fee_wallet` and `fee_bps` from config. If `fee_bps` is `0` or `fee_wallet` is unset, no commission is applied.

Examples:
- No config → no commission.
- `fee_wallet` + `fee_bps: 40`, no service → fixed 0.4% on every payout.
- Service configured → service decides per client; config is the fallback.

---

## Properties

| Property | Value |
|---|---|
| Atomicity | Commission and payout execute in a single on-chain transaction |
| Opt-in | No config = no commission |
| Fail-safe | Service failure falls back to config values |
| No custody | Fee transfers go directly on-chain to `fee_wallet` |
| Chain scope | Polygon only. Asset Hub: not applicable in v1 |
| Refunds | Not charged commission |
| Auditability | Both transfers share one transaction hash, verifiable on Polygonscan |

---

## Future Considerations

- **Asset Hub commission**: requires a different mechanism (batch extrinsic, not EIP-7702)
- **Commission reporting**: aggregate fee revenue query / dashboard
- **Fee splitting**: multiple fee recipients per payout by extending the batch
