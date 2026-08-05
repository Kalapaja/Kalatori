# Swaps

Decision records and behavioral notes for the swaps subsystem
(`daemon/src/swaps/`, `daemon/src/clients/swaps/`). Not yet a full subsystem
overview — see [architecture.md](architecture.md) for the component map.

## Payer signatures are validated and claimed atomically (2026-08-04)

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
  the **stored** quote before `claim_swap_for_submission`. The executor named
  in the request body is not trusted for this — the stored one is what drives
  submission, so validating against anything else would let a caller pick an
  executor that validates nothing.
- Signature persistence and the `Created → Submitted` transition happen in one
  conditional `UPDATE ... WHERE id = ? AND status = 'Created' RETURNING *`.
  Exactly one caller can claim the row and reach the provider. A replay or race
  loser reads the row's current status and returns **409
  `SWAP_ALREADY_SUBMITTED`**; a missing row still returns not found.
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

## Unusable gas parameters are forwarded as absent (2026-08-04)

Across does not omit `gas` when its simulation fails. A production
`/api/swap/approval` response with `simulationSuccess: false` instead carried
`gas: "0"`; the fee caps and `value` were genuinely omitted. The daemon treats
an Across gas limit of zero exactly like an absent one. It does the same for a
zero `maxFeePerGas` and drops `maxPriorityFeePerGas` whenever the cap is absent
or normalized away. A zero priority fee remains valid and is preserved when
the max fee cap is non-zero.

0x remains separate because its contract differs, not because zero is
acceptable there. 0x types `gas` as genuinely nullable rather than using zero
as a sentinel, and its own troubleshooting guide warns that wallet-side
`eth_estimateGas` can come in *too low* to settle — so a supplied limit is
forwarded verbatim and only an absent one is omitted. A zero limit from 0x
would be exactly as unmineable as Across's; there is no evidence it emits one,
and normalizing on that speculation would risk discarding a limit 0x
deliberately sized.

The daemon publishes unusable or absent fields with their keys **omitted from
the JSON**. Absent means "estimate it yourself": Kassette passes `undefined`,
and viem drops undefined gas fields from `eth_sendTransaction` so the payer's
wallet estimates them.

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
containing #50** — an older Kassette still throws on an omitted key. Satisfied
as of 0.9.4, which pins front-end 0.1.0 (`front-end.mk`); do not lower that pin
below 0.1.0 while the daemon omits gas keys.

`UnusableQuote` itself remains, for quote expiry timestamps outside the
representable range (`daemon/src/clients/swaps/across/types.rs`,
`daemon/src/clients/swaps/bungee/types.rs`). It stays internal rather than
becoming a provider rejection: nothing the requester does differently fixes it.

Related: `simulationSuccess` is **not** a rejection signal and is never read.
Across marks it optional and omits it entirely on `/swap/gasless`, so it is
deserialized with a default purely so an absent field doesn't fail the whole
quote. Across's own SDK ignores the flag and re-simulates locally.

## Submission-attempt protocol for backend-submitted swaps (2026-08-03)

`SwapsExecutor::submit_with_signature` atomically persists the payer signature
and `Submitted` (without a transaction hash) **before** calling the external
executor, then attaches the hash afterwards via `update_swap_transaction_hash`,
which never changes status:
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
  incoming funds are again caught by the transfer subscription. When the
  tracker reloads this terminal hashless row, it emits the manual-reconciliation
  warning once and drops the row from its in-memory store. Hashless non-terminal
  rows and database read errors remain tracked for the next polling round.
- If the hash write fails after a successful submission, the caller still gets
  `Ok` — a retry could double-submit — but it receives the post-`Submitted` row,
  never the pre-submission one, so the reported status matches what was
  persisted.
