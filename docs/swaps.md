# Swaps

Decision records and behavioral notes for the swaps subsystem
(`daemon/src/swaps/`, `daemon/src/clients/swaps/`). Not yet a full subsystem
overview — see [architecture.md](architecture.md) for the component map.

## Payer signatures are validated against the stored quote (2026-08-04)

> **Known gap:** submission is not atomic. Two concurrent requests for the same
> swap can both read `Created`, both persist a signature, both transition to
> `Submitted` (the status trigger permits `Submitted → Submitted`), and both
> call the provider — risking duplicate external execution and a row whose
> signature, hash and error come from different requests. The fix is a
> conditional `UPDATE ... WHERE id = ? AND status = 'Created' RETURNING` so only
> one request may submit. Not addressed here.

A gasless 0x quote can require two signatures — the trade and a token approval
— and the payer submits them as one `"<trade>|<approval>"` string through the
unauthenticated `POST /public/swap/signature`. The submission path used to
`split_once("|").unwrap()`, so a single signature for an approval-requiring
quote persisted an unsplittable value and then panicked, taking the daemon with
it ([#349](https://github.com/Kalapaja/Kalatori/issues/349)).

The shape of the payload is now checked before it is written:

- `SwapsClient::validate_signature` is a per-executor hook, defaulting to
  accept-anything. Only `ZeroExGaslessClient` overrides it, because it is the
  only executor whose signature payload has internal structure.
- `SwapsExecutor::submit_with_signature` loads the swap and validates against
  the **stored** quote before `update_swap_set_signature`. The executor named
  in the request body is not trusted for this — the stored one is what drives
  submission, so validating against anything else would let a caller pick an
  executor that validates nothing.
- Validation and submission share `split_gasless_signature`, so a payload
  accepted at the door cannot fail to split later. The submission path still
  returns the error rather than unwrapping, because rows written before this
  landed may already hold an unsplittable value.
- Each component must also be `0x`-prefixed even-length hex. 0x's schema types
  `signatureBytes` only as `string` and does not mandate the prefix, so this is
  deliberately **narrower** than the provider's contract; every value 0x's own
  examples and reference clients produce carries the prefix, and the only
  gasless signature the daemon generates comes from `const_hex::encode_prefixed`.
- **Shape validation is not authenticity.** `0x00|0x00` passes it. Such a
  payload is persisted, moves the swap to `Submitted`, and is then rejected by
  the provider, which marks the swap terminally `Failed` — so anyone holding a
  swap UUID can still destroy a `Created` gasless swap, and for those payloads
  the 400's "re-sign and retry" promise does not hold. Closing that needs the
  signature verified against the quote and payer before persistence, or
  provider invalid-signature responses classified as re-signable so the swap
  stays `Created`. Neither is done yet.
- Whether an approval signature is required is derived from `approval` being
  present in the stored quote. 0x documents `approval != null` as gasless
  approval being *available*, with `issues.allowance` indicating whether it is
  *needed*; the daemon does not parse `issues` and the submission path has
  always required both signatures whenever `approval` is present, so validation
  matches submission. Reconciling both with `issues.allowance` is open work.
- Rejection surfaces as `SwapsClientError::InvalidSignaturePayload` →
  `SwapsExecutorError::InvalidSignature` → `SwapRequestError::InvalidSignature`
  → **400 `INVALID_SWAP_SIGNATURE`**. It is the submitter's to fix by
  re-signing, so it must not be reported as an internal failure.

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
