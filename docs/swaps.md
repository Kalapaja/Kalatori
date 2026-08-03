# Swaps

Decision records and behavioral notes for the swaps subsystem
(`daemon/src/swaps/`, `daemon/src/clients/swaps/`). Not yet a full subsystem
overview — see [architecture.md](architecture.md) for the component map.

## Unusable provider quotes are rejected, not forwarded (2026-08-04)

When a provider returns a quote without usable gas parameters — Across with a
failed simulation, 0x with no gas estimate — the daemon rejects it with
`SwapsClientError::UnusableQuote` rather than passing it on.

The alternative (forward the quote with the gas fields omitted and let the
signing wallet estimate them) was considered and rejected: Kassette submits
those fields unguarded via `BigInt(swapTx.gas)`, so an omitted field throws in
the payer's browser, and substituting `0` publishes a transaction with a zero
gas limit that cannot be mined. Neither is publishable.

Once Kassette accepts absent gas (Kalapaja/Kassette#49) these can be passed
through as optional instead of rejected. The rationale lives next to the code in
`daemon/src/clients/swaps/across/types.rs` and
`daemon/src/clients/swaps/zeroex.rs`.

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
