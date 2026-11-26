import { writeFile } from 'node:fs/promises';

export async function connectCdp(port = 9223, commandTimeoutMs = 20000) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find(t => t.type === 'page');
  if (!target) throw new Error('No page target');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  let nextId = 0;
  const pending = new Map();
  const listeners = new Map();
  const errors = [];
  const requests = [];
  ws.addEventListener('message', event => {
    const data = JSON.parse(event.data);
    if (data.id) {
      const item = pending.get(data.id);
      if (!item) return;
      pending.delete(data.id);
      clearTimeout(item.timer);
      if (data.error) item.reject(new Error(`${item.method}: ${data.error.message}`));
      else item.resolve(data.result);
      return;
    }
    if (data.method === 'Runtime.exceptionThrown') errors.push(data.params);
    if (data.method === 'Network.requestWillBeSent') requests.push(data.params.request.url);
    for (const resolve of listeners.get(data.method) || []) resolve(data.params);
    listeners.delete(data.method);
  });
  function send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`CDP timeout: ${method}`));
      }, commandTimeoutMs);
      pending.set(id, { resolve, reject, timer, method });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }
  function once(method, timeout = 20000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        listeners.delete(method);
        reject(new Error(`Event timeout: ${method}`));
      }, timeout);
      listeners.set(method, [params => {
        clearTimeout(timer);
        resolve(params);
      }]);
    });
  }
  async function evaluate(expression) {
    const result = await send('Runtime.evaluate', {
      expression,
      returnByValue: true,
      awaitPromise: true,
      userGesture: true,
    });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
    }
    return result.result?.value;
  }
  async function waitFor(expression, timeout = 15000) {
    const started = Date.now();
    while (Date.now() - started < timeout) {
      if (await evaluate(`Boolean(${expression})`)) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error(`waitFor timeout: ${expression}`);
  }
  async function navigate(url) {
    const loaded = once('Page.loadEventFired');
    await send('Page.navigate', { url });
    await loaded;
  }
  async function screenshot(path) {
    const result = await send('Page.captureScreenshot', {
      format: 'png',
      captureBeyondViewport: false,
      fromSurface: true,
    });
    await writeFile(path, Buffer.from(result.data, 'base64'));
  }
  await send('Page.enable');
  await send('Runtime.enable');
  await send('Network.enable');
  await send('Emulation.setDeviceMetricsOverride', {
    width: 1440,
    height: 960,
    deviceScaleFactor: 1,
    mobile: false,
  });
  function close() {
    for (const item of pending.values()) clearTimeout(item.timer);
    pending.clear();
    ws.close();
  }
  return { send, evaluate, waitFor, navigate, screenshot, close, errors, requests };
}

export function assert(condition, message) {
  if (!condition) throw new Error(`ASSERTION FAILED: ${message}`);
  console.log(`PASS ${message}`);
}
