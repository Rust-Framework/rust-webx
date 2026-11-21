const http = require('http');
const fs = require('fs');
const { chromium } = require('playwright-core');

const executablePath = process.env.CHROME_PATH || 'C:/Users/lusid/AppData/Local/ms-playwright/chromium-1255/chrome-win64/chrome.exe';
const regChallenge = Buffer.from('webx-browser-registration-challenge-32b').toString('base64url');
const authChallenge = Buffer.from('webx-browser-assertion-challenge--32b').toString('base64url');

const html = `<!doctype html>
<meta charset="utf-8">
<title>webx webauthn e2e</title>
<pre id="status">starting</pre>
<script>
const b64u = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf)))
  .replace(/\\+/g, '-').replace(/\\//g, '_').replace(/=+$/, '');
const unb64u = (s) => {
  s = s.replace(/-/g, '+').replace(/_/g, '/');
  while (s.length % 4) s += '=';
  return Uint8Array.from(atob(s), c => c.charCodeAt(0));
};
async function post(kind, data) {
  const r = await fetch('/capture', {method:'POST', headers:{'content-type':'application/json'}, body: JSON.stringify({kind, ...data})});
  if (!r.ok) throw new Error(await r.text());
}
(async () => {
  try {
    const cred = await navigator.credentials.create({publicKey: {
      challenge: unb64u('${regChallenge}'),
      rp: {id: 'localhost', name: 'webx test'},
      user: {id: new Uint8Array([1,2,3,4,5,6,7,8]), name: 'alice', displayName: 'Alice'},
      pubKeyCredParams: [{type:'public-key', alg:-7}],
      timeout: 20000,
      attestation: 'none',
      authenticatorSelection: {userVerification:'required', residentKey:'required'}
    }});
    await post('registration', {
      clientDataJSON: b64u(cred.response.clientDataJSON),
      attestationObject: b64u(cred.response.attestationObject),
      rawId: b64u(cred.rawId)
    });
    const asrt = await navigator.credentials.get({publicKey: {
      challenge: unb64u('${authChallenge}'),
      rpId: 'localhost',
      allowCredentials: [{type:'public-key', id: cred.rawId}],
      userVerification: 'required',
      timeout: 20000
    }});
    await post('assertion', {
      clientDataJSON: b64u(asrt.response.clientDataJSON),
      authenticatorData: b64u(asrt.response.authenticatorData),
      signature: b64u(asrt.response.signature),
      rawId: b64u(asrt.rawId)
    });
    document.getElementById('status').textContent = 'DONE';
    window.__webx_done = true;
  } catch (e) {
    document.getElementById('status').textContent = 'ERROR ' + e;
    window.__webx_error = String(e);
  }
})();
</script>`;

let captured = [];
const server = http.createServer((req, res) => {
  if (req.method === 'GET') {
    res.writeHead(200, {'content-type': 'text/html'});
    res.end(html);
    return;
  }
  if (req.method === 'POST' && req.url === '/capture') {
    let body = '';
    req.on('data', c => body += c);
    req.on('end', () => {
      try {
        captured.push(JSON.parse(body));
        res.writeHead(204); res.end();
      } catch (e) { res.writeHead(400); res.end(String(e)); }
    });
    return;
  }
  res.writeHead(404); res.end();
});

(async () => {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  const browser = await chromium.launch({ headless: true, executablePath });
  const context = await browser.newContext();
  const page = await context.newPage();
  const client = await context.newCDPSession(page);
  await client.send('WebAuthn.enable');
  await client.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2', transport: 'internal',
      hasResidentKey: true, hasUserVerification: true,
      isUserVerified: true, automaticPresenceSimulation: true
    }
  });
  await page.goto(`http://localhost:${port}/`, {waitUntil: 'domcontentloaded'});
  for (let i = 0; i < 80 && captured.length < 2; i++) await page.waitForTimeout(100);
  const error = await page.evaluate(() => window.__webx_error || null);
  await browser.close();
  server.close();
  if (captured.length !== 2) {
    console.error('capture failed: count=' + captured.length + ' error=' + error);
    process.exit(2);
  }
  const registration = captured.find(x => x.kind === 'registration');
  const assertion = captured.find(x => x.kind === 'assertion');
  fs.writeFileSync('browser_webauthn.json', JSON.stringify({
    rp_id: 'localhost',
    origin: `http://localhost:${port}`,
    reg_challenge: regChallenge,
    auth_challenge: authChallenge,
    registration, assertion
  }, null, 2));
  console.log('captured registration + assertion on ' + `http://localhost:${port}`);
})();
