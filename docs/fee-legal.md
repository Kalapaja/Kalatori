# Commission System — Money Flow

This document describes the proposed commission mechanism for Kalatori, a non-custodial blockchain payment gateway. It defines the parties involved, the flow of funds, the control model for keys and addresses, and the rules governing commission collection.

## Parties

| Party | Role |
|---|---|
| **Customer** | Pays for goods by sending the invoice amount to a payment address |
| **Merchant** | Owns the store; receives all payouts to their own wallet; backs up the seed phrase |
| **Daemon** | Manages private keys for temporary payment addresses; executes payouts autonomously |
| **Provider** | Supplies PaaS infrastructure on which the daemon runs; does not operate or control the daemon |

---

## Money Flow

The daemon runs on the provider's infrastructure. The provider supplies compute and hosting but has no access to the daemon's keys or business logic.

```
── 1. Invoice creation ──────────────────────────────────────────────────────

  Merchant                        Daemon
     │                               │
     ├── POST /order ──────────────► │  derives address[n] from seed (BIP-39)
     │                               │  stores invoice in DB
     │◄── { payment_address[n], ──── │
     │      amount, expiry }         │

  Merchant shares payment_address[n] and amount with Customer

── 2. Customer payment ──────────────────────────────────────────────────────

  Customer                        Chain
     │                               │
     ├── 100.00 USDC ───────────────►│ → payment_address[n]
     │                               │

── 3. Sweep (triggered on payment detection) ────────────────────────────────

  Daemon                          Chain
     │                               │
     │  detects incoming tx          │
     │  resolves commission rate     │  (commission service → config fallback)
     │                               │
     │  builds EIP-4337 UserOp       │
     │  executeBatch  ─────────────► │  atomic: all-or-nothing
     │                               │
     │                               ├──  ~0.10 USDC ──► Pimlico/Circle paymaster  (gas)
     │                               ├──  99.50 USDC ──► Merchant wallet
     │                               └──   0.40 USDC ──► Provider fee wallet        (0.4% commission)
     │                               │
     │  marks invoice paid in DB     │
     │  address[n]: balance = 0,     │
     │  never reused                 │
```

> **Isolation:** Technical isolation of the daemon from provider infrastructure via Confidential Containers is planned but not yet implemented. Until then, separation is contractual and operational only.

---

## Keys and Addresses

### Seed phrase

The daemon generates a BIP-39 seed on first startup. All temporary payment addresses are derived from this seed. The merchant backs up the seed; it is not transmitted to any other party.

- **Self-hosted:** seed lives on the merchant's own machine.
- **PaaS / hosted:** seed lives on the provider's infrastructure; the merchant backs it up. The provider has no intended access — analogous to AWS running hardware without inspecting tenant data. Confidential Containers will enforce this technically once implemented.

### Temporary payment addresses

One unique address per invoice, derived from the seed. Single-use: the daemon sweeps the full balance immediately after payment is confirmed. The address holds no funds after payout and is never reused.

### Merchant wallet

Configured by the merchant; independent of the seed; can be any external address. The provider cannot change it.

### Provider fee wallet

The address to which the commission portion of each payout is sent. Placed in the daemon config by the merchant as a condition of using the provider's PaaS. The provider verifies this address matches their own before agreeing to run the daemon.

---

## Commission

### How it works

Commission is a basis-point percentage of the net-of-gas payout, deducted before the merchant transfer. Both the merchant transfer and the commission transfer are batched into a single atomic EIP-4337 `executeBatch` call — either both land or neither does.

```
Invoice:            100.00 USDC
Gas:                 ~0.10 USDC  →  paymaster
Net after gas:       99.90 USDC
Commission (0.4%):    0.40 USDC  →  provider fee wallet
Merchant receives:   99.50 USDC  →  merchant wallet
```

The customer always pays exactly the invoice amount. Commission reduces only what the merchant receives.

### Configuration

Commission is configured by the merchant in the daemon's `commission.json`. As a condition of using the provider's PaaS, the merchant sets three fields to values dictated by the provider:

| Field | Description |
|---|---|
| `fee_service_url` | URL of the provider's commission service (used for per-payout rate resolution) |
| `fee_wallet` | Provider's address to receive commission transfers |
| `fee_bps` | Fallback commission rate in basis points (used if the service is unreachable) |

The provider verifies all three fields match their own values before agreeing to run the daemon. If any field is missing, mismatched, or below the agreed rate, the provider rejects the deployment.

The provider never injects config into the daemon. The merchant owns the config; the provider only checks that it satisfies the service agreement.

```json
{
  "fee_service_url": "https://commission.provider.example/resolve",
  "fee_wallet": "0xProviderFeeWalletAddress",
  "fee_bps": 40
}
```

### Rate resolution (per payout)

1. **Commission service** — if configured, the daemon POSTs `{ client_id, chain, amount }` to the service URL. `{ apply: true, fee_wallet, fee_bps }` → used as-is. `{ apply: false }` → no commission.
2. **Config fallback** — if no service URL is set, or the call fails, `fee_wallet` and `fee_bps` are read from `commission.json`.
3. **No commission** — if `fee_bps` is `0` or `fee_wallet` is absent, the payout is a single transfer with no commission split.

### Hard cap

The daemon enforces a hard maximum of **100 bps (1%)**. Any rate above this — whether from the service or config — is clamped to 1% before the transaction is built. This is enforced in daemon code and cannot be overridden externally.
