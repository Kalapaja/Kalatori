# @kalatori/webhook-simulator

Test your webhook endpoint against Kalatori's exact signing and delivery behavior.

Sends properly signed HMAC-SHA256 webhook requests via a local proxy (no CORS issues), matching Kalatori's production format. Reports what would happen in production for each response status.

## Quick Start

```bash
npx @kalatori/webhook-simulator
```

This starts a local web UI (default: http://localhost:16726) and opens it in your browser.

### Options

```
--port PORT   Port to listen on (default: 16726)
--self-test   Run HMAC test vectors against Rust reference implementation and exit
--help        Show usage
```

## Features

- **HMAC-SHA256 signing** matching Kalatori's exact algorithm (`METHOD\nPATH\nBODY\nTIMESTAMP`)
- **Server-side proxy** — requests go through Node.js, not the browser, so there are no CORS restrictions
- **Event type presets** — generates realistic payloads for all invoice lifecycle events (created, paid, expired, etc.)
- **Request log** with expandable request/response details and production behavior notes
- **Self-test mode** — validates the HMAC implementation against test vectors generated from Kalatori's Rust code
- **Zero dependencies** — only uses Node.js built-in modules

## Webhook Signature Format

Kalatori signs webhooks with two headers:

| Header | Description |
|--------|-------------|
| `X-KALATORI-SIGNATURE` | Hex-encoded HMAC-SHA256 of the message below |
| `X-KALATORI-TIMESTAMP` | Unix timestamp (seconds) used in the signature |

The signed message is constructed as:

```
POST\n/your/webhook/path\n{"json":"body"}\n1706745600
```

That is: `METHOD`, `PATH`, `BODY`, and `TIMESTAMP` joined by literal newline characters.

## Verifying Signatures (receiver side)

Pseudocode for your webhook handler:

```python
import hmac, hashlib, time

MAX_SKEW_SECONDS = 300  # 5 minutes

def verify(request, secret):
    signature = request.headers["X-KALATORI-SIGNATURE"]
    timestamp = request.headers["X-KALATORI-TIMESTAMP"]

    # Reject stale or far-future timestamps to limit replay window
    if abs(time.time() - int(timestamp)) > MAX_SKEW_SECONDS:
        return False

    message = f"{request.method}\n{request.path}\n{request.body}\n{timestamp}"
    expected = hmac.new(secret.encode(), message.encode(), hashlib.sha256).hexdigest()
    return hmac.compare_digest(signature, expected)
```

## Event Types

| Event | Statuses |
|-------|----------|
| `created` | Waiting |
| `updated` | Waiting |
| `paid` | Paid, OverPaid |
| `partially_paid` | PartiallyPaid |
| `expired` | UnpaidExpired, PartiallyPaidExpired |
| `admin_canceled` | AdminCanceled |
| `customer_canceled` | CustomerCanceled |

## Requirements

Node.js >= 18.0.0

## License

GPL-3.0
