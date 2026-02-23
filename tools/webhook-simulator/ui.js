'use strict';

function getHtml() {
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Kalatori Webhook Simulator</title>
<style>
*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; line-height: 1.5; color: #1a1a1a; background: #f5f5f5; padding: 24px; max-width: 900px; margin: 0 auto; }
h1 { font-size: 1.5rem; margin-bottom: 4px; }
.subtitle { color: #666; font-size: 0.9rem; margin-bottom: 20px; }
.section { background: #fff; border: 1px solid #ddd; border-radius: 8px; padding: 20px; margin-bottom: 16px; }
.section h2 { font-size: 1.1rem; margin-bottom: 12px; border-bottom: 1px solid #eee; padding-bottom: 8px; }
label { display: block; font-weight: 600; font-size: 0.85rem; margin-bottom: 4px; color: #333; }
input[type="text"], input[type="password"], select { width: 100%; padding: 8px 12px; border: 1px solid #ccc; border-radius: 4px; font-size: 0.9rem; font-family: inherit; }
.secret-row { display: flex; gap: 8px; }
.secret-row input { flex: 1; }
.btn-toggle { padding: 8px 12px; border: 1px solid #ccc; border-radius: 4px; background: #f9fafb; cursor: pointer; font-size: 0.8rem; white-space: nowrap; color: #555; }
.btn-toggle:hover { background: #e5e7eb; }
input:focus, select:focus, textarea:focus { outline: none; border-color: #4a90d9; box-shadow: 0 0 0 2px rgba(74,144,217,0.2); }
.field { margin-bottom: 12px; }
.row { display: flex; gap: 12px; }
.row > .field { flex: 1; }
textarea#payload-editor { width: 100%; min-height: 300px; font-family: "SF Mono", "Fira Code", "Fira Mono", "Roboto Mono", monospace; font-size: 0.82rem; padding: 12px; border: 1px solid #ccc; border-radius: 4px; resize: vertical; line-height: 1.4; tab-size: 2; }
textarea#payload-editor.invalid { border-color: #d32f2f; background: #fff5f5; }
.btn { display: inline-block; padding: 10px 20px; border: none; border-radius: 4px; font-size: 0.9rem; font-weight: 600; cursor: pointer; transition: background 0.15s; }
.btn-primary { background: #2563eb; color: #fff; }
.btn-primary:hover { background: #1d4ed8; }
.btn-primary:disabled { background: #93c5fd; cursor: not-allowed; }
.btn-secondary { background: #e5e7eb; color: #333; }
.btn-secondary:hover { background: #d1d5db; }
.btn-small { padding: 6px 12px; font-size: 0.8rem; }
.btn-row { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; }
#result-panel { display: none; }
.result-status { font-size: 1.1rem; font-weight: 700; padding: 8px 0; }
.result-status.success { color: #16a34a; }
.result-status.failure { color: #dc2626; }
.result-detail { margin-bottom: 8px; }
.result-detail summary { cursor: pointer; font-weight: 600; font-size: 0.85rem; color: #555; }
.result-detail pre { background: #f8f8f8; border: 1px solid #e5e5e5; border-radius: 4px; padding: 10px; font-size: 0.8rem; overflow-x: auto; margin-top: 4px; white-space: pre-wrap; word-break: break-all; }
.production-note { background: #fffbeb; border: 1px solid #fbbf24; border-radius: 4px; padding: 12px; margin-top: 12px; font-size: 0.85rem; line-height: 1.5; }
.production-note strong { color: #92400e; }
.info-box { background: #eff6ff; border: 1px solid #bfdbfe; border-radius: 4px; padding: 12px; font-size: 0.85rem; margin-bottom: 16px; }
.error-text { color: #dc2626; font-size: 0.8rem; margin-top: 4px; }
.spinner { display: inline-block; width: 16px; height: 16px; border: 2px solid #93c5fd; border-top-color: #2563eb; border-radius: 50%; animation: spin 0.6s linear infinite; vertical-align: middle; margin-right: 6px; }
@keyframes spin { to { transform: rotate(360deg); } }
.log-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; border-bottom: 1px solid #eee; padding-bottom: 8px; }
.log-header h2 { margin-bottom: 0; border-bottom: none; padding-bottom: 0; }
.log-count { font-size: 0.75rem; font-weight: 500; color: #666; background: #e5e7eb; border-radius: 10px; padding: 1px 8px; margin-left: 8px; vertical-align: middle; }
.log-entry { border: 1px solid #e5e5e5; border-radius: 6px; padding: 12px; margin-bottom: 10px; border-left: 4px solid #d1d5db; }
.log-entry.success { border-left-color: #16a34a; }
.log-entry.failure { border-left-color: #dc2626; }
.log-entry-header { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; flex-wrap: wrap; }
.log-badge { font-size: 0.75rem; font-weight: 700; padding: 2px 8px; border-radius: 4px; white-space: nowrap; }
.log-badge.success { background: #dcfce7; color: #166534; }
.log-badge.failure { background: #fee2e2; color: #991b1b; }
.log-badge.pending { background: #e0e7ff; color: #3730a3; }
.log-url { font-family: "SF Mono", "Fira Code", monospace; font-size: 0.8rem; color: #555; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
.log-time { font-size: 0.78rem; color: #888; white-space: nowrap; }
.log-entry .production-note { margin-top: 8px; }
</style>
</head>
<body>

<h1>Kalatori Webhook Simulator</h1>
<p class="subtitle">Test your webhook endpoint against Kalatori's exact signing and delivery behavior.</p>

<div class="info-box">
  This tool sends a single webhook request with a properly signed HMAC-SHA256 payload, matching
  Kalatori's production behavior. Requests are sent server-side (via local proxy), so there are no
  CORS restrictions &mdash; just like production. It does <strong>not</strong> retry on failure &mdash;
  instead it reports what would happen in production.
</div>

<!-- Configuration -->
<div class="section">
  <h2>Configuration</h2>
  <div class="field">
    <label for="webhook-url">Webhook URL</label>
    <input type="text" id="webhook-url" placeholder="https://your-server.com/webhooks/invoices" value="http://localhost:8000/webhooks/invoices">
  </div>
  <div class="field">
    <label for="secret-key">HMAC Secret Key</label>
    <div class="secret-row">
      <input type="password" id="secret-key" placeholder="your-shared-secret" value="secret">
      <button type="button" class="btn-toggle" id="toggle-secret" onclick="toggleSecret()">Show</button>
    </div>
  </div>
</div>

<!-- Event Selection -->
<div class="section">
  <h2>Event</h2>
  <div class="row">
    <div class="field">
      <label for="event-type">Event Type</label>
      <select id="event-type">
        <option value="created">created</option>
        <option value="updated">updated</option>
        <option value="paid">paid</option>
        <option value="partially_paid">partially_paid</option>
        <option value="expired">expired</option>
        <option value="admin_canceled">admin_canceled</option>
        <option value="customer_canceled">customer_canceled</option>
      </select>
    </div>
    <div class="field">
      <label for="invoice-status">Invoice Status</label>
      <select id="invoice-status">
        <option value="Waiting">Waiting</option>
      </select>
    </div>
  </div>
</div>

<!-- Payload Editor -->
<div class="section">
  <h2>Payload</h2>
  <div class="btn-row">
    <button class="btn btn-secondary btn-small" onclick="regeneratePayload()">Regenerate Payload</button>
    <button class="btn btn-secondary btn-small" onclick="formatPayload()">Format JSON</button>
    <span id="json-error" class="error-text"></span>
  </div>
  <textarea id="payload-editor" spellcheck="false"></textarea>
</div>

<!-- Send -->
<div style="margin-bottom: 16px;">
  <button class="btn btn-primary" id="send-btn" onclick="sendWebhook()">Send Webhook</button>
</div>

<!-- Request Log -->
<div class="section" id="log-section" style="display:none">
  <div class="log-header">
    <h2>Request Log <span id="log-count" class="log-count"></span></h2>
    <button class="btn btn-secondary btn-small" onclick="clearLog()">Clear Log</button>
  </div>
  <div id="request-log"></div>
</div>

<script>
// ============================================================================
// Event Type -> Status Mapping
// ============================================================================
const EVENT_STATUS_MAP = {
  created:            [{ value: 'Waiting', label: 'Waiting' }],
  updated:            [{ value: 'Waiting', label: 'Waiting' }],
  admin_canceled:     [{ value: 'AdminCanceled', label: 'AdminCanceled' }],
  customer_canceled:  [{ value: 'CustomerCanceled', label: 'CustomerCanceled' }],
  paid:               [{ value: 'Paid', label: 'Paid' }, { value: 'OverPaid', label: 'OverPaid' }],
  partially_paid:     [{ value: 'PartiallyPaid', label: 'PartiallyPaid' }],
  expired:            [{ value: 'UnpaidExpired', label: 'UnpaidExpired' }, { value: 'PartiallyPaidExpired', label: 'PartiallyPaidExpired' }],
};

// ============================================================================
// UUID v4 Generator
// ============================================================================
function uuidv4() {
  return crypto.randomUUID();
}

// ============================================================================
// HMAC-SHA256 Signing (matches client/src/utils.rs calculate_hmac exactly)
//
// Message format: METHOD\\nPATH\\nBODY\\nTIMESTAMP
// Output: hex-encoded HMAC-SHA256
// ============================================================================
async function computeSignature(secret, method, path, body, timestamp) {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    'raw',
    enc.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign']
  );
  const message = method + '\\n' + path + '\\n' + body + '\\n' + timestamp;
  const sig = await crypto.subtle.sign('HMAC', key, enc.encode(message));
  return Array.from(new Uint8Array(sig))
    .map(b => b.toString(16).padStart(2, '0'))
    .join('');
}

// ============================================================================
// Payload Generation
// ============================================================================
function generatePayload(eventType, status) {
  const now = new Date();
  const createdAt = new Date(now.getTime() - 3600000);
  const validTill = new Date(createdAt.getTime() + 86400000);
  const invoiceId = uuidv4();
  const eventId = uuidv4();
  const amount = '100.00';

  let totalReceived = '0';
  let transactions = [];

  if (status === 'Paid') {
    totalReceived = amount;
    transactions = [makeSampleTransaction(invoiceId, amount, now)];
  } else if (status === 'OverPaid') {
    totalReceived = '150.00';
    transactions = [makeSampleTransaction(invoiceId, '150.00', now)];
  } else if (status === 'PartiallyPaid') {
    totalReceived = '50.00';
    transactions = [makeSampleTransaction(invoiceId, '50.00', now)];
  } else if (status === 'PartiallyPaidExpired') {
    totalReceived = '50.00';
    transactions = [makeSampleTransaction(invoiceId, '50.00', createdAt)];
  }

  const event = {
    id: eventId,
    event_entity: 'invoice',
    event_type: eventType,
    payload: {
      id: invoiceId,
      order_id: 'order-' + Math.floor(Math.random() * 100000),
      asset_name: 'USDT',
      asset_id: '1984',
      chain: 'PolkadotAssetHub',
      amount: amount,
      payment_address: '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY',
      status: status,
      payment_url: 'https://app.kalatori.com/invoice/' + invoiceId,
      redirect_url: 'https://example.com/thank-you',
      cart: {
        items: [{
          name: 'Widget Pro',
          quantity: 2,
          price: '50.00'
        }]
      },
      total_received_amount: totalReceived,
      transactions: transactions,
      valid_till: validTill.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
      created_at: createdAt.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
      updated_at: now.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
    },
    timestamp: now.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
  };

  return event;
}

function makeSampleTransaction(invoiceId, amount, date) {
  let blockNumber = 12345678 + Math.floor(Math.random() * 1000);
  let positionInBlock = Math.floor(Math.random() * 10);
  return {
    id: uuidv4(),
    invoice_id: invoiceId,
    block_number: blockNumber,
    position_in_block: positionInBlock,
    tx_hash: '0x' + Array.from(crypto.getRandomValues(new Uint8Array(32)))
      .map(b => b.toString(16).padStart(2, '0')).join(''),
    transaction_type: 'Incoming',
    asset_name: 'USDT',
    asset_id: '1984',
    chain: 'PolkadotAssetHub',
    amount: amount,
    source_address: '5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy',
    destination_address: '5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY',
    created_at: date.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
    updated_at: date.toISOString().replace(/\\.\\d{3}Z$/, 'Z'),
    status: 'Completed',
    transaction_link: 'https://assethub-polkadot.subscan.io/extrinsic/' + blockNumber + '-' + positionInBlock
  };
}

// ============================================================================
// UI Logic
// ============================================================================
const eventTypeSelect = document.getElementById('event-type');
const statusSelect = document.getElementById('invoice-status');
const payloadEditor = document.getElementById('payload-editor');
const jsonError = document.getElementById('json-error');

function updateStatusOptions() {
  const eventType = eventTypeSelect.value;
  const statuses = EVENT_STATUS_MAP[eventType] || [];
  statusSelect.innerHTML = '';
  for (const s of statuses) {
    const opt = document.createElement('option');
    opt.value = s.value;
    opt.textContent = s.label;
    statusSelect.appendChild(opt);
  }
}

function regeneratePayload() {
  const eventType = eventTypeSelect.value;
  const status = statusSelect.value;
  const payload = generatePayload(eventType, status);
  payloadEditor.value = JSON.stringify(payload, null, 2);
  payloadEditor.classList.remove('invalid');
  jsonError.textContent = '';
}

function formatPayload() {
  try {
    const parsed = JSON.parse(payloadEditor.value);
    payloadEditor.value = JSON.stringify(parsed, null, 2);
    payloadEditor.classList.remove('invalid');
    jsonError.textContent = '';
  } catch (e) {
    payloadEditor.classList.add('invalid');
    jsonError.textContent = 'Invalid JSON: ' + e.message;
  }
}

eventTypeSelect.addEventListener('change', () => {
  updateStatusOptions();
  regeneratePayload();
});

statusSelect.addEventListener('change', () => {
  regeneratePayload();
});

// Initialize
updateStatusOptions();
regeneratePayload();

// ============================================================================
// Toggle Secret Visibility
// ============================================================================
function toggleSecret() {
  const input = document.getElementById('secret-key');
  const btn = document.getElementById('toggle-secret');
  if (input.type === 'password') {
    input.type = 'text';
    btn.textContent = 'Hide';
  } else {
    input.type = 'password';
    btn.textContent = 'Show';
  }
}

// ============================================================================
// Send Webhook (via local proxy — no CORS issues)
// ============================================================================
let attemptCounter = 0;

function addLogEntry(id, badgeText, badgeClass, url, requestText, responseText, productionHtml) {
  const logSection = document.getElementById('log-section');
  const logContainer = document.getElementById('request-log');
  const logCount = document.getElementById('log-count');

  logSection.style.display = '';

  // Collapse details on previously-newest entry
  const prevNewest = logContainer.firstElementChild;
  if (prevNewest) {
    prevNewest.querySelectorAll('details[open]').forEach(d => d.removeAttribute('open'));
  }

  const entry = document.createElement('div');
  entry.className = 'log-entry ' + (badgeClass || '');
  entry.id = 'log-entry-' + id;

  const timeStr = new Date().toLocaleTimeString();

  entry.innerHTML =
    '<div class="log-entry-header">' +
      '<span class="log-badge ' + (badgeClass || 'pending') + '">' + escapeHtml(badgeText) + '</span>' +
      '<span class="log-url">POST ' + escapeHtml(url) + '</span>' +
      '<span class="log-time">#' + id + ' at ' + timeStr + '</span>' +
    '</div>' +
    '<details class="result-detail" open>' +
      '<summary>Request</summary>' +
      '<pre>' + escapeHtml(requestText || '') + '</pre>' +
    '</details>' +
    '<details class="result-detail" open>' +
      '<summary>Response</summary>' +
      '<pre>' + escapeHtml(responseText || '') + '</pre>' +
    '</details>' +
    (productionHtml ? '<div class="production-note">' + productionHtml + '</div>' : '');

  logContainer.prepend(entry);

  const count = logContainer.children.length;
  logCount.textContent = count + (count === 1 ? ' request' : ' requests');

  entry.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
}

function updateLogEntry(id, badgeText, badgeClass, requestText, responseText, productionHtml) {
  const entry = document.getElementById('log-entry-' + id);
  if (!entry) return;

  entry.className = 'log-entry ' + (badgeClass || '');

  const badge = entry.querySelector('.log-badge');
  if (badge) {
    badge.className = 'log-badge ' + (badgeClass || 'pending');
    badge.textContent = badgeText;
  }

  const pres = entry.querySelectorAll('details.result-detail pre');
  if (pres[0] && requestText !== undefined) pres[0].textContent = requestText;
  if (pres[1] && responseText !== undefined) pres[1].textContent = responseText;

  const note = entry.querySelector('.production-note');
  if (productionHtml) {
    if (note) {
      note.innerHTML = productionHtml;
    } else {
      const div = document.createElement('div');
      div.className = 'production-note';
      div.innerHTML = productionHtml;
      entry.appendChild(div);
    }
  }

  entry.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
}

function clearLog() {
  document.getElementById('request-log').innerHTML = '';
  document.getElementById('log-count').textContent = '';
  document.getElementById('log-section').style.display = 'none';
  attemptCounter = 0;
}

function escapeHtml(str) {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

async function sendWebhook() {
  attemptCounter++;
  const attempt = attemptCounter;

  const sendBtn = document.getElementById('send-btn');
  const url = document.getElementById('webhook-url').value.trim();
  const secret = document.getElementById('secret-key').value;
  const body = payloadEditor.value;

  // Validate inputs
  if (!url) {
    addLogEntry(attempt, 'Validation Error', 'failure', '(empty)', '', 'Webhook URL is empty. Please enter a URL above.', '');
    return;
  }

  let parsedUrl;
  try {
    parsedUrl = new URL(url);
  } catch {
    addLogEntry(attempt, 'Validation Error', 'failure', url, '', 'Invalid URL format: "' + url + '"', '');
    return;
  }

  try {
    JSON.parse(body);
  } catch (e) {
    payloadEditor.classList.add('invalid');
    jsonError.textContent = 'Invalid JSON: ' + e.message;
    addLogEntry(attempt, 'Validation Error', 'failure', url, '', 'Payload is not valid JSON:\\n' + e.message, '');
    return;
  }

  payloadEditor.classList.remove('invalid');
  jsonError.textContent = '';

  sendBtn.disabled = true;
  sendBtn.innerHTML = '<span class="spinner"></span>Sending...';

  const path = parsedUrl.pathname;
  const timestamp = Math.floor(Date.now() / 1000).toString();

  let signature;
  try {
    signature = await computeSignature(secret, 'POST', path, body, timestamp);
  } catch (e) {
    addLogEntry(attempt, 'Signing Error', 'failure', url, '', 'Failed to compute HMAC signature: ' + e.message, '');
    sendBtn.disabled = false;
    sendBtn.textContent = 'Send Webhook';
    return;
  }

  const requestText =
    'POST ' + url + '\\n' +
    'Content-Type: application/json\\n' +
    'X-KALATORI-SIGNATURE: ' + signature + '\\n' +
    'X-KALATORI-TIMESTAMP: ' + timestamp + '\\n' +
    '\\nBody:\\n' + body + '\\n' +
    '\\nHMAC signed message (for debugging):\\n' +
    'POST\\\\n' + path + '\\\\n' + body + '\\\\n' + timestamp;

  addLogEntry(attempt, 'Sending...', 'pending', url, requestText, '(waiting for response...)', '');

  const startTime = performance.now();

  try {
    const proxyResponse = await fetch('/api/proxy', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        url: url,
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-KALATORI-SIGNATURE': signature,
          'X-KALATORI-TIMESTAMP': timestamp,
        },
        body: body,
      }),
    });

    const result = await proxyResponse.json();

    if (result.error === 'timeout') {
      updateLogEntry(attempt, 'Timeout', 'failure',
        requestText,
        'Request timed out after 60 seconds.\\nTime: ' + result.elapsed + 'ms',
        '<strong>Production behavior:</strong> ' +
        'Request timeout (60 seconds). ' +
        'In production, Kalatori would continuously retry this event until it succeeds. ' +
        'All subsequent webhook events for this same invoice would be held in queue ' +
        'until this one succeeds.'
      );
    } else if (result.error === 'connection_error') {
      updateLogEntry(attempt, 'Connection Error', 'failure',
        requestText,
        'Error: ' + result.message + '\\nTime: ' + result.elapsed + 'ms',
        '<strong>Production behavior:</strong> ' +
        'Connection failed. In production, Kalatori would continuously retry this event until it succeeds. ' +
        'All subsequent webhook events for this same invoice ' +
        'would be held in queue until this one succeeds.'
      );
    } else if (result.status !== undefined) {
      const isSuccess = result.status >= 200 && result.status < 300;
      const responseBody = result.responseBody || '';

      const responseText =
        'Status: ' + result.status + ' ' + result.statusText + '\\n' +
        'Time: ' + result.elapsed + 'ms\\n\\n' +
        'Body:\\n' + (responseBody.length > 2000 ? responseBody.slice(0, 2000) + '\\n... (truncated)' : responseBody);

      let productionHtml;
      if (isSuccess) {
        productionHtml =
          '<strong>Production behavior:</strong> ' +
          'This event would be marked as delivered and removed from the queue. ' +
          'No retries.';
      } else {
        productionHtml =
          '<strong>Production behavior:</strong> ' +
          'Server responded with <strong>' + result.status + '</strong>. ' +
          'In production, Kalatori would continuously retry this event until it succeeds. ' +
          'All subsequent webhook events for this same invoice would be held in queue ' +
          'until this one succeeds (FIFO ordering per entity). ' +
          'Up to 10 concurrent webhook deliveries are allowed across different invoices.';
      }

      updateLogEntry(attempt,
        result.status + ' ' + result.statusText,
        isSuccess ? 'success' : 'failure',
        requestText, responseText, productionHtml
      );
    } else {
      updateLogEntry(attempt, 'Unexpected Error', 'failure',
        requestText,
        'Unexpected proxy response: ' + JSON.stringify(result), ''
      );
    }
  } catch (e) {
    const elapsed = Math.round(performance.now() - startTime);
    updateLogEntry(attempt, 'Proxy Error', 'failure',
      requestText,
      'Error communicating with local proxy: ' + e.message + '\\nTime: ' + elapsed + 'ms\\n\\n' +
      'Make sure the webhook simulator server is still running.',
      ''
    );
  }

  sendBtn.disabled = false;
  sendBtn.textContent = 'Send Webhook';
}
</script>

</body>
</html>`;
}

module.exports = { getHtml };
