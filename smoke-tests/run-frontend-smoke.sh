#!/bin/zsh
set -euo pipefail

cd "$(dirname "$0")"
test_file="${1:-./frontend-smoke.mjs}"
[[ -f "$test_file" ]] || { print -u2 "Frontend test not found: $test_file"; exit 1; }
if [[ -n "${CSM_CDP_PORT:-}" ]]; then
  port="$CSM_CDP_PORT"
else
  port="$(node -e 'const s = require("node:net").createServer(); s.listen(0, "127.0.0.1", () => { console.log(s.address().port); s.close(); });')"
fi
chrome="${CSM_CHROME_PATH:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[[ -x "$chrome" ]] || { print -u2 "Chrome executable not found: $chrome"; exit 1; }
profile="$(mktemp -d "${TMPDIR%/}/csm-headless-chrome.XXXXXX")"
chrome_pid=""

cleanup() {
  if [[ -n "$chrome_pid" ]]; then
    kill -TERM "$chrome_pid" 2>/dev/null || true
    wait "$chrome_pid" 2>/dev/null || true
  fi
  rm -rf "$profile"
}
trap cleanup EXIT INT TERM

CSM_CDP_PORT="$port" node -e '
  const net = require("node:net");
  const port = Number(process.env.CSM_CDP_PORT);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error("Invalid CDP port");
  const server = net.createServer();
  server.once("error", error => { throw new Error(`CDP port ${port} is unavailable: ${error.message}`); });
  server.listen(port, "127.0.0.1", () => server.close());
'

"$chrome" \
  --headless=new \
  --remote-debugging-port="$port" \
  --user-data-dir="$profile" \
  --no-first-run \
  --no-default-browser-check \
  --disable-background-networking \
  --disable-component-update \
  --disable-extensions \
  --disable-sync \
  --metrics-recording-only \
  about:blank >"$profile/chrome.log" 2>&1 &
chrome_pid=$!

CSM_CDP_PORT="$port" node --input-type=module -e '
  const port = Number(process.env.CSM_CDP_PORT);
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) { ready = true; break; }
    } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!ready) throw new Error("Headless Chrome did not expose CDP");
'

CSM_CDP_PORT="$port" node "$test_file"
