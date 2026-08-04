# Swaps

Decision records and behavioral notes for the swaps subsystem
(`daemon/src/swaps/`, `daemon/src/clients/swaps/`). Not yet a full subsystem
overview — see [architecture.md](architecture.md) for the component map.

## Absent gas parameters are forwarded as absent (2026-08-04)

When a provider returns a quote without gas parameters — Across with a failed
simulation, 0x with no gas estimate — the daemon publishes the quote with those
keys **omitted from the JSON**. Absent means "estimate it yourself": Kassette
converts with `!= null ? BigInt(…) : undefined` since Kalapaja/Kassette#50, and
viem drops undefined gas fields from `eth_sendTransaction` so the payer's wallet
estimates them.

Omission is load-bearing in both directions. Sending `0` would publish a
transaction with a zero gas limit or zero fee caps, which cannot be mined.
Sending `null` relies on Kassette's null check rather than the field simply not
being there; the daemon serializes with `skip_serializing_if` so neither the
key nor a null reaches the browser.

Which fields are optional differs by provider, and follows what Kassette
accepts. The key names below are the ones the daemon **publishes** — the quote
structs carry no `rename_all`, so they are snake_case, and Kassette's types
match. Do not confuse them with the providers' inbound camelCase (`maxFeePerGas`,
`gasPrice`), which only ever appears on the deserializing structs:

| Provider | Optional (omitted when absent) | Always present |
|---|---|---|
| Across | `gas`, `max_fee_per_gas`, `max_priority_fee_per_gas` | `value` (absent ⇒ `0`, per Across's docs and Kassette's `0n` default) |
| 0x | `gas` | `gas_price`, `value` (not nullable in 0x's v2 spec) |

This supersedes the earlier decision (same day) to reject such quotes with
`SwapsClientError::UnusableQuote`. That rejection existed only because Kassette
converted the fields unguarded via `BigInt(swapTx.gas)`, which threw in the
payer's browser when a field was absent. Kassette#50 fixed that half; this is
the other half. **The relaxed daemon must not ship ahead of a Kassette build
containing #50** — an older Kassette still throws on an omitted key.

`UnusableQuote` itself remains, for quote expiry timestamps outside the
representable range (`daemon/src/clients/swaps/across/types.rs`,
`daemon/src/clients/swaps/bungee/types.rs`). It stays internal rather than
becoming a provider rejection: nothing the requester does differently fixes it.

Related: `simulationSuccess` is **not** a rejection signal and is never read.
Across marks it optional and omits it entirely on `/swap/gasless`, so it is
deserialized with a default purely so an absent field doesn't fail the whole
quote. Across's own SDK ignores the flag and re-simulates locally.

## Submission-attempt protocol for backend-submitted swaps (2026-08-03)

`SwapsExecutor::submit_with_signature` persists `Submitted` (without a
transaction hash) **before** calling the external executor, then attaches the
hash afterwards via `update_swap_transaction_hash`, which never changes status:
the swaps tracker polls the database on a 100 ms interval and may already have
moved the swap `Submitted → Pending`, and the SQLite status trigger forbids
going back. Consequences:

- A crash between submission and hash persistence leaves a `Submitted`/`Pending`
  swap without a hash. The tracker re-reads such swaps each round and, when the
  hash is still missing, logs a `warn!` naming the swap — that is the signal for
  manual reconciliation. (It must not be `debug!`: production runs at `info`,
  where the swap would otherwise fail silently forever.) Funds that actually
  arrive are still detected by the chain transfer subscription.
- A rejected submission marks the swap `Failed` with the executor's error
  message. If the rejection was spurious (e.g. a timeout after acceptance),
  incoming funds are again caught by the transfer subscription.
- If the hash write fails after a successful submission, the caller still gets
  `Ok` — a retry could double-submit — but it receives the post-`Submitted` row,
  never the pre-submission one, so the reported status matches what was
  persisted.
