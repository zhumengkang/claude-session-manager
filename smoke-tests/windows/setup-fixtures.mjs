import fs from 'node:fs';
import path from 'node:path';
import { randomBytes, randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';

const root = process.argv[2];
if (!root || process.platform !== 'win32') throw new Error('Usage: setup-fixtures.mjs NEW_WINDOWS_DIRECTORY');
fs.mkdirSync(root);
const runId = `CSM-WIN-${Date.now()}-${randomBytes(3).toString('hex')}`;
const claudeDir = path.join(root, 'claude-config');
const codexDir = path.join(root, 'codex-home');
const cacheDir = path.join(root, 'cache');
const webviewDir = path.join(root, 'webview2');
const projectsDir = path.join(claudeDir, 'projects');
const projectA = path.join(root, "项目 space;&'A");
const projectB = path.join(root, '项目 B');
for (const dir of [projectsDir, codexDir, cacheDir, projectA, projectB]) fs.mkdirSync(dir, { recursive: true });
const token = randomBytes(24).toString('hex');
const sessions = [];
const timestamp = index => new Date(Date.UTC(2026, 10, 24, 1, index)).toISOString();

for (const [index, name] of ['Alpha', 'Beta', 'Gamma'].entries()) {
  const id = randomUUID();
  const cwd = index === 0 ? projectA : projectB;
  const slug = index === 0 ? 'project-a' : 'project-b';
  const dir = path.join(projectsDir, slug);
  fs.mkdirSync(dir, { recursive: true });
  const filePath = path.join(dir, `${id}.jsonl`);
  const title = `${runId} Claude ${name}`;
  const count = index === 0 ? 205 : index === 1 ? 1005 : 2;
  const lines = [{ type: 'custom-title', customTitle: title, timestamp: timestamp(0), cwd }];
  for (let i = 0; i < count; i++) {
    const role = i % 2 ? 'assistant' : 'user';
    const text = i === count - 1 && index === 0
      ? 'Latest **Windows** message\n```js\nconst verified = true;\n```'
      : `Synthetic ${name} message ${i + 1}`;
    lines.push({ type: role, timestamp: timestamp(i + 1), cwd, message: { content: text } });
  }
  fs.writeFileSync(filePath, lines.map(line => JSON.stringify(line)).join('\n') + '\n');
  sessions.push({ provider: 'claude', id, title, cwd, filePath, favorite: index < 2 });
}

const failureSessions = ['ResumeFailure', 'TrustPending'].map((name, index) => {
  const id = randomUUID();
  return {
    provider: 'claude', id, title: `${runId} Claude ${name}`, cwd: projectB,
    filePath: path.join(projectsDir, 'project-b', `${id}.jsonl`),
    launchBehavior: index === 0 ? 'exit' : 'trust',
  };
});

const db = new DatabaseSync(path.join(codexDir, 'state_5.sqlite'));
db.exec(`CREATE TABLE threads (
  id TEXT PRIMARY KEY, rollout_path TEXT, created_at INTEGER, updated_at INTEGER,
  source TEXT, cwd TEXT, title TEXT, first_user_message TEXT, name TEXT,
  preview TEXT, archived INTEGER NOT NULL DEFAULT 0
)`);
const insert = db.prepare('INSERT INTO threads VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)');
for (const [index, name] of ['Delta', 'Epsilon'].entries()) {
  const id = randomUUID();
  const cwd = index === 0 ? projectA : projectB;
  const dir = path.join(codexDir, 'sessions', '2026', '11', '24');
  fs.mkdirSync(dir, { recursive: true });
  const filePath = path.join(dir, `rollout-2026-11-24T00-00-00-${id}.jsonl`);
  const title = `${runId} Codex ${name}`;
  const events = [
    { type: 'response_item', timestamp: timestamp(0), payload: { type: 'message', role: 'user', content: [{ type: 'input_text', text: `Synthetic ${name} question` }] } },
    { type: 'response_item', timestamp: timestamp(1), payload: { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: `Synthetic ${name} **answer**` }] } },
  ];
  fs.writeFileSync(filePath, events.map(event => JSON.stringify(event)).join('\n') + '\n');
  const ts = 1795479600 + index;
  const dbCwd = index === 1 ? '\\\\?\\UNC\\server\\share\\项目 UNC' : cwd;
  const source = index === 0 ? 'cli' : 'vscode';
  insert.run(id, filePath, ts, ts, source, dbCwd, title, '', '', '');
  sessions.push({ provider: 'codex', id, title, cwd: dbCwd, filePath, source, favorite: index === 0 });
}
db.close();
fs.writeFileSync(path.join(claudeDir, 'favorites.json'), JSON.stringify(sessions.filter(s => s.favorite).map(s => s.provider === 'codex' ? `codex:${s.id}` : s.id)));

const cliDir = path.join(root, "cli tools\\O'Brien");
const scriptDir = path.join(root, 'scripts & fixtures');
fs.mkdirSync(cliDir, { recursive: true });
fs.mkdirSync(scriptDir);
const providerScript = path.join(scriptDir, 'provider-fixture.cjs');
fs.copyFileSync(new URL('./provider-fixture.cjs', import.meta.url), providerScript);
for (const provider of ['claude', 'codex']) {
  const trustGuard = provider === 'claude'
    ? 'if /I "%~3"=="%CSM_FIXTURE_TRUST_ID%" (echo Synthetic Trust prompt pending - provider not started & pause & exit /b 0)\r\n'
    : '';
  fs.writeFileSync(path.join(cliDir, `${provider}.cmd`), `@echo off\r\n${trustGuard}"${process.execPath}" "${providerScript}" ${provider} %*\r\n`);
}
const config = {
  root, runId, token, claudeDir, codexDir, cacheDir, webviewDir, cliDir,
  nodeHome: path.dirname(process.execPath), nodeExe: process.execPath, providerScript,
  daemonExe: path.join(codexDir, 'packages', 'app-server-daemon', 'releases', '0.0.0-smoke-aarch64-pc-windows-msvc', 'bin', 'codex.exe'),
  logPath: path.join(root, 'providers.jsonl'),
  debugLog: path.join(root, 'launch.log'), sessions,
  failureSessions, trustSessionId: failureSessions[1].id,
};
fs.mkdirSync(path.dirname(config.daemonExe), { recursive: true });
fs.copyFileSync(process.execPath, config.daemonExe);
const configPath = path.join(root, 'fixture.json');
fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
console.log(JSON.stringify({ configPath, runId, sessionCount: sessions.length }, null, 2));
