#!/usr/bin/env node
'use strict';

const args = process.argv.slice(2);
let port = 16726;
let selfTest = false;

for (let i = 0; i < args.length; i++) {
  if (args[i] === '--port' && args[i + 1]) {
    port = parseInt(args[i + 1], 10);
    if (Number.isNaN(port) || port < 1 || port > 65535) {
      console.error('Invalid port number: ' + args[i + 1]);
      process.exit(1);
    }
    i++;
  } else if (args[i] === '--self-test') {
    selfTest = true;
  } else if (args[i] === '--help' || args[i] === '-h') {
    console.log('Usage: kalatori-webhook-simulator [--port PORT] [--self-test]');
    console.log('');
    console.log('Options:');
    console.log('  --port PORT   Port to listen on (default: 16726)');
    console.log('  --self-test   Run HMAC test vectors and exit');
    process.exit(0);
  }
}

if (selfTest) {
  runSelfTest();
} else {
  startServer(port);
}

function runSelfTest() {
  const crypto = require('node:crypto');
  const vectors = require('./test-vectors');

  let passed = 0;
  let failed = 0;

  for (let i = 0; i < vectors.length; i++) {
    const tv = vectors[i];
    const message = tv.method + '\n' + tv.path + '\n' + tv.body + '\n' + tv.timestamp;
    const computed = crypto.createHmac('sha256', tv.secret)
      .update(message)
      .digest('hex');

    if (computed === tv.expected_signature) {
      passed++;
      console.log('  #' + (i + 1) + ' PASS');
    } else {
      failed++;
      console.error('  #' + (i + 1) + ' FAIL — expected ' +
        tv.expected_signature.slice(0, 16) + '..., got ' + computed.slice(0, 16) + '...');
    }
  }

  console.log('');
  if (failed === 0) {
    console.log('All ' + passed + ' tests passed. HMAC implementation matches Kalatori.');
    process.exit(0);
  } else {
    console.error(failed + ' of ' + (passed + failed) + ' tests failed.');
    process.exit(1);
  }
}

function startServer(port) {
  const { createServer } = require('./server');
  const server = createServer();

  server.listen(port, '127.0.0.1', () => {
    const url = 'http://localhost:' + port;
    console.log('Kalatori Webhook Simulator running at ' + url);
    console.log('Press Ctrl+C to stop.\n');

    // Open browser automatically
    const { exec } = require('child_process');
    const platform = process.platform;
    const cmd = platform === 'darwin' ? 'open'
      : platform === 'win32' ? 'start'
      : 'xdg-open';
    exec(cmd + ' ' + url, () => {
      // Ignore errors — browser may not be available (e.g. headless server)
    });
  });

  process.on('SIGINT', () => {
    console.log('\nShutting down...');
    server.close(() => process.exit(0));
  });

  process.on('SIGTERM', () => {
    server.close(() => process.exit(0));
  });
}
