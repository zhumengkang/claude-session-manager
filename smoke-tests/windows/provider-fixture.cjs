const fs = require('node:fs');
const path = require('node:path');

const provider = process.argv[2];
const args = process.argv.slice(3);
const configPath = process.env.CSM_FIXTURE_CONFIG;
if (!configPath || !['claude', 'codex'].includes(provider)) process.exit(64);
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
const expected = provider === 'claude'
  ? ['--dangerously-skip-permissions', '-r']
  : ['--dangerously-bypass-approvals-and-sandbox', 'resume'];
const daemonSessionId = process.env.CSM_FIXTURE_DAEMON_SESSION_ID || '';
const daemonMode = provider === 'codex' && daemonSessionId.length > 0;
const validArguments = daemonMode
  ? args.length === 3 && args[0] === 'app-server' && args[1] === '--fixture-session' && args[2] === daemonSessionId
  : args.length === 3 && args[0] === expected[0] && args[1] === expected[1];
if (!validArguments) {
  console.error('Unexpected provider arguments');
  process.exit(65);
}
const sessionId = daemonMode ? daemonSessionId : args[2];
const owned = [...config.sessions, ...(config.failureSessions || [])].find(s => s.provider === provider && s.id === sessionId);
if (!owned) process.exit(66);
if (owned.launchBehavior === 'exit') process.exit(67);

let rolloutHandle;
if (provider === 'claude') {
  const sessionsDir = path.join(config.claudeDir, 'sessions');
  fs.mkdirSync(sessionsDir, { recursive: true });
  fs.writeFileSync(path.join(sessionsDir, `${process.pid}.json`), JSON.stringify({
    sessionId, pid: process.pid, updatedAt: Date.now(), startedAt: Date.now(),
  }));
} else {
  rolloutHandle = fs.openSync(owned.filePath, 'r+');
}

fs.appendFileSync(config.logPath, JSON.stringify({
  pid: process.pid,
  ppid: process.ppid,
  provider,
  daemon: daemonMode,
  sessionId,
  args,
  cwd: process.cwd(),
  environmentInherited: process.env.CSM_FIXTURE_TOKEN === config.token,
  parentMarkersCleared: !process.env.CLAUDE_CODE_CHILD_SESSION && !process.env.CLAUDECODE,
  startedAt: new Date().toISOString(),
}) + '\n');

function close() {
  if (rolloutHandle !== undefined) fs.closeSync(rolloutHandle);
  process.exit(0);
}
process.on('SIGTERM', close);
process.on('SIGINT', close);
if (owned.launchBehavior === 'exit-after-ms') {
  const exitAfterMs = Number(owned.exitAfterMs);
  if (!Number.isInteger(exitAfterMs) || exitAfterMs < 100 || exitAfterMs > 10_000) process.exit(68);
  setTimeout(close, exitAfterMs);
} else {
  setInterval(() => {}, 1000);
}
