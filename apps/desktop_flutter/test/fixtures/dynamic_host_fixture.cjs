// Real loopback/process fixture for the desktop's CLI readiness handshake.
const fs = require('node:fs');
const http = require('node:http');
const crypto = require('node:crypto');

const [mode, home, fakePid, ...args] = process.argv.slice(2);
if (mode === 'exit') process.exit(23);
const option = name => args.includes(name) ? args[args.indexOf(name) + 1] : undefined;
const readyFile = option('--ready-file');
const stdioLog = option('--stdio-log');
if (stdioLog) fs.appendFileSync(stdioLog, 'fixture started\n');
const instanceId = crypto.randomUUID();
let reportedHome = home;
const server = http.createServer((request, response) => {
  if (stdioLog) fs.appendFileSync(stdioLog, 'fixture request after startup\n');
  if (request.url === '/__fixture/stop') {
    if (request.headers['x-dsh-fixture-instance'] !== instanceId) {
      response.writeHead(403).end();
      return;
    }
    response.end('stopping', () => server.close(() => process.exit(0)));
    return;
  }
  if (request.url === '/__fixture/home') {
    if (request.headers['x-dsh-fixture-instance'] !== instanceId) {
      response.writeHead(403).end();
      return;
    }
    let body = '';
    request.on('data', chunk => body += chunk);
    request.on('end', () => {
      reportedHome = JSON.parse(body).home;
      response.end('home changed');
    });
    return;
  }
  let body = '';
  request.on('data', chunk => body += chunk);
  request.on('end', () => {
    const message = JSON.parse(body);
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({
      type: 'server-response', rpcId: message.rpcId,
      result: {ok: true, value: {
        version: 'fixture', home: reportedHome, cwd: reportedHome,
        processId: mode === 'rpc-wrong-pid' ? process.pid + 1 : process.pid,
        instanceId,
      }},
    }));
  });
});
server.listen(Number(option('--port')), '127.0.0.1', () => {
  if (mode === 'timeout') return;
  const ready = {
    version: 1,
    pid: mode === 'wrong-pid' ? Number(fakePid) : process.pid,
    instanceId: mode === 'wrong-instance' ? crypto.randomUUID() : instanceId,
    url: mode === 'wrong-url' ? 'http://127.0.0.1:0' :
      `http://127.0.0.1:${server.address().port}`,
    executable: process.execPath,
    home,
  };
  const contents = mode === 'malformed' ? '{broken' :
    mode === 'oversized' ? ' '.repeat(4097) : JSON.stringify(ready);
  fs.writeFileSync(readyFile + '.tmp', contents, {flag: 'wx'});
  fs.renameSync(readyFile + '.tmp', readyFile);
});
