'use strict';

const http = require('node:http');
const https = require('node:https');
const { getHtml } = require('./ui');

function createServer() {
  return http.createServer((req, res) => {
    if (req.method === 'GET' && (req.url === '/' || req.url === '/index.html')) {
      const html = getHtml();
      res.writeHead(200, {
        'Content-Type': 'text/html; charset=utf-8',
        'Content-Length': Buffer.byteLength(html),
      });
      res.end(html);
      return;
    }

    if (req.method === 'POST' && req.url === '/api/proxy') {
      let body = '';
      req.on('data', (chunk) => { body += chunk; });
      req.on('end', () => {
        handleProxy(body, res);
      });
      return;
    }

    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('Not Found');
  });
}

function handleProxy(rawBody, res) {
  let proxyReq;
  try {
    proxyReq = JSON.parse(rawBody);
  } catch {
    res.writeHead(400, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'Invalid JSON in proxy request' }));
    return;
  }

  const { url, method, headers, body } = proxyReq;

  if (!url || !method) {
    res.writeHead(400, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'Missing url or method' }));
    return;
  }

  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    res.writeHead(400, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'Invalid URL: ' + url }));
    return;
  }

  const transport = parsed.protocol === 'https:' ? https : http;
  const startTime = Date.now();

  const options = {
    hostname: parsed.hostname,
    port: parsed.port || (parsed.protocol === 'https:' ? 443 : 80),
    path: parsed.pathname + parsed.search,
    method: method,
    headers: headers || {},
    timeout: 60000,
  };

  const proxyRequest = transport.request(options, (proxyResponse) => {
    let responseBody = '';
    proxyResponse.on('data', (chunk) => { responseBody += chunk; });
    proxyResponse.on('end', () => {
      const elapsed = Date.now() - startTime;

      const responseHeaders = {};
      for (const [key, value] of Object.entries(proxyResponse.headers)) {
        responseHeaders[key] = value;
      }

      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        status: proxyResponse.statusCode,
        statusText: proxyResponse.statusMessage || '',
        responseBody: responseBody,
        elapsed: elapsed,
        responseHeaders: responseHeaders,
      }));
    });
  });

  proxyRequest.on('timeout', () => {
    proxyRequest.destroy();
    const elapsed = Date.now() - startTime;
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
      error: 'timeout',
      elapsed: elapsed,
      message: 'Request timed out after 60 seconds',
    }));
  });

  proxyRequest.on('error', (err) => {
    const elapsed = Date.now() - startTime;
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
      error: 'connection_error',
      elapsed: elapsed,
      message: err.message,
    }));
  });

  if (body) {
    proxyRequest.write(body);
  }
  proxyRequest.end();
}

module.exports = { createServer };
