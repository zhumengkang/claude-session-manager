import fs from 'node:fs';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { connectCdp, assert } from '../cdp.mjs';

const configPath = path.resolve(process.argv[2] || '');
const appPath = path.resolve(process.argv[3] || '');
if (process.platform !== 'win32' || !fs.existsSync(configPath) || !fs.existsSync(appPath)) {
  throw new Error('Usage: windows-gui-smoke.mjs FIXTURE_JSON CSM_APP_EXE');
}
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
const here = path.dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.CSM_WINDOWS_CDP_PORT || 19337);
const env = {
  ...process.env,
  PATH: `${config.cliDir};${config.nodeHome};${process.env.PATH}`,
  CLAUDE_CONFIG_DIR: config.claudeDir,
  CODEX_HOME: config.codexDir,
  CSM_CACHE_DIR: config.cacheDir,
  CSM_DEBUG_LOG: config.debugLog,
  CSM_FIXTURE_CONFIG: configPath,
  CSM_FIXTURE_TOKEN: config.token,
  CSM_FIXTURE_TRUST_ID: config.trustSessionId || '',
  WEBVIEW2_USER_DATA_FOLDER: config.webviewDir,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
};
const appLog = fs.openSync(path.join(config.root, 'app-process.log'), 'a');
const app = spawn(appPath, [], { env, stdio: ['ignore', appLog, appLog], windowsHide: false });
fs.closeSync(appLog);
let appSpawnError;
let appTerminationRequested = false;
app.once('error', error => { appSpawnError ||= error; });
const appExited = () => app.exitCode !== null || app.signalCode !== null;
const waitForAppExit = timeoutMs => {
  if (appExited()) return Promise.resolve(true);
  return new Promise(resolve => {
    const onExit = () => { clearTimeout(timer); resolve(true); };
    const timer = setTimeout(() => {
      app.removeListener('exit', onExit);
      resolve(appExited());
    }, timeoutMs);
    app.once('exit', onExit);
  });
};
let cdp;
const candidateExportFiles = new Set();
let batchAttempted = false;
let failure;
const exportBeta = config.sessions.find(s => s.provider === 'claude' && s.title.endsWith('Beta'));
const isOwnedExport = (file, expectedSession) => {
  if (!file || !path.isAbsolute(file)) return false;
  const basename = path.basename(file);
  if (path.basename(path.dirname(file)) !== 'Claude Session Manager Exports'
    || !basename.startsWith(config.runId) || !basename.endsWith('.md')) return false;
  const owner = config.sessions.find(session => basename.includes(session.id.slice(0, 8)));
  return !!owner && (!expectedSession || owner.id === expectedSession.id);
};
const isExternalHttp = value => {
  try {
    const url = new URL(value);
    return (url.protocol === 'http:' || url.protocol === 'https:')
      && url.hostname !== 'ipc.localhost'
      && url.hostname !== 'tauri.localhost';
  } catch {
    return false;
  }
};
const runPowerShell = (script, provider, sessionId = '', artifactName = '') => execFileSync('powershell.exe', [
  '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(here, script), '-ConfigPath', configPath,
  ...(provider ? ['-Provider', provider] : []),
  ...(sessionId ? ['-SessionId', sessionId] : []),
  ...(artifactName ? ['-ArtifactName', artifactName] : []),
], { encoding: 'utf8', timeout: 45000, windowsHide: true });
const providerEntries = () => fs.existsSync(config.logPath)
  ? fs.readFileSync(config.logPath, 'utf8').split(/\r?\n/).filter(Boolean).flatMap(line => {
    try { return [JSON.parse(line)]; } catch { return []; }
  })
  : [];
const daemonChildren = [];
const startDaemon = session => {
  const state = { child: null, error: null, sessionId: session.id };
  state.child = spawn(config.daemonExe, [config.providerScript, 'codex', 'app-server', '--fixture-session', session.id], {
    cwd: config.root,
    env: { ...env, CSM_FIXTURE_DAEMON_SESSION_ID: session.id },
    stdio: 'ignore', windowsHide: true,
  });
  state.child.once('error', error => { state.error = error; });
  daemonChildren.push(state);
  return state;
};
const waitForDaemon = async state => {
  for (let attempt = 0; attempt < 40; attempt++) {
    if (state.error) throw state.error;
    const entry = providerEntries().find(item => item.daemon && item.sessionId === state.sessionId);
    if (entry) {
      assert(entry.pid === state.child.pid, 'Daemon log identity matches the exact owned child PID');
      return entry;
    }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error(`Owned daemon did not open its rollout: ${state.sessionId}`);
};
try {
  fs.writeFileSync(path.join(config.root, 'app-owner.json'), JSON.stringify({ pid: app.pid, appPath, startedAt: new Date().toISOString() }));
  let connected = false;
  for (let i = 0; i < 120 && !connected; i++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      if (response.ok && (await response.json()).some(target => target.type === 'page')) connected = true;
    } catch {}
    if (appSpawnError) break;
    if (!connected) await new Promise(resolve => setTimeout(resolve, 250));
  }
  if (appSpawnError) throw appSpawnError;
  assert(connected, 'Actual Windows WebView2 exposes its dedicated CDP port');
  cdp = await connectCdp(port, 40000);
  await cdp.waitFor(`typeof providerStates !== 'undefined' && providerStates.claude.loaded && document.querySelectorAll('.session-item').length === 3`, 30000);
  assert(await cdp.evaluate(`IS_WINDOWS && MODIFIER_LABEL === 'Ctrl'`), 'Real Windows UA enables Ctrl shortcuts');
  assert(await cdp.evaluate(`providerStates.claude.allSessions.every(s => ${JSON.stringify(config.runId)} && s.title.startsWith(${JSON.stringify(config.runId)}))`), 'Claude list reads only the isolated fixture titles');
  const renderedProjects = await cdp.evaluate(`[...document.querySelectorAll('.session-item .s-project')].map(el => el.textContent)`);
  const expectedProjects = config.sessions.filter(s => s.provider === 'claude').map(s => path.basename(s.cwd));
  assert(renderedProjects.length === 3 && expectedProjects.every(name => renderedProjects.includes(name)), 'Windows backslash project paths render as folder names instead of full paths');
  fs.writeFileSync(path.join(config.root, 'network-requests.json'), JSON.stringify(cdp.requests, null, 2));
  assert(cdp.requests.every(url => !isExternalHttp(url)), 'Application loads no runtime CDN resources');
  const c1 = config.sessions.find(s => s.provider === 'claude' && s.title.endsWith('Alpha'));
  await cdp.evaluate(`window.__testCalls=[]; window.__originalInvoke=invoke; invoke=async (cmd,args={}) => { try { const result=await window.__originalInvoke(cmd,args); window.__testCalls.push({cmd,args,result}); return result; } catch (error) { window.__testCalls.push({cmd,args,error:String(error)}); throw error; } }; true`);
  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(c1.id)}] .s-body').click()`);
  await cdp.waitFor(`activeSession?.session_id === ${JSON.stringify(c1.id)} && chatLoaded && chatMsgs.length === 200`);
  assert(await cdp.evaluate(`chatMsgs.at(-1).text.includes('Latest **Windows** message')`), 'Detail initially positions at the newest Windows fixture message');
  assert(await cdp.evaluate(`!!document.querySelector('#chat-area pre') && !!document.querySelector('#chat-area strong')`), 'Markdown and code blocks render in Windows WebView2');
  const initialSnapshotLen = await cdp.evaluate(`chatSnapshotLen`);
  assert(Number.isSafeInteger(initialSnapshotLen) && initialSnapshotLen > 0, 'Windows detail captures the real backend snapshot length');
  fs.appendFileSync(c1.filePath, JSON.stringify({ type: 'assistant', timestamp: new Date().toISOString(), cwd: c1.cwd, message: { content: 'Appended after the fixed Windows snapshot' } }) + '\n');
  await cdp.evaluate(`document.getElementById('load-more-btn').click()`);
  await cdp.waitFor(`chatMsgs.length === 205 && !chatHasMore && chatTotalExact`);
  const olderPageCall = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'get_messages' && c.args.filePath === ${JSON.stringify(c1.filePath)}).at(-1)`);
  assert(olderPageCall?.args.snapshotLen === initialSnapshotLen, 'Windows older-page IPC reuses the original snapshot after a real file append');
  assert(await cdp.evaluate(`new Set(chatMsgs.map(m => m.text)).size === 205`), 'Older-page loading preserves all 205 unique messages after a real append');
  assert(!(await cdp.evaluate(`chatMsgs.some(m => m.text === 'Appended after the fixed Windows snapshot')`)), 'Real appended content is excluded from the current fixed snapshot');
  await cdp.screenshot(path.join(config.root, 'windows-app.png'));

  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(exportBeta.id)}]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await cdp.evaluate(`document.getElementById('ctx-export').click()`);
  await cdp.waitFor(`window.__testCalls.some(c => c.cmd === 'export_session_markdown' && c.result?.message_count === 1005)`, 30000);
  const exportCall = await cdp.evaluate(`window.__testCalls.find(c => c.cmd === 'export_session_markdown')`);
  const longExportFile = exportCall.result?.file_path;
  if (isOwnedExport(longExportFile, exportBeta)) candidateExportFiles.add(longExportFile);
  fs.writeFileSync(path.join(config.root, 'export-result.json'), JSON.stringify(exportCall.result, null, 2));
  assert(!exportCall.result?.cleanup_warning, `Long export cleans its temporary workspace without a warning: ${exportCall.result?.cleanup_warning || 'none'}`);
  assert(isOwnedExport(longExportFile, exportBeta), 'Long export returns a unique file owned by this fixture run');
  const leftoverWorkspaces = fs.readdirSync(path.dirname(longExportFile), { withFileTypes: true })
    .filter(entry => entry.isDirectory() && entry.name.startsWith(`.csm-export-${app.pid}-`));
  assert(leftoverWorkspaces.length === 0, 'No temporary export workspace from this app process remains on the shared Documents filesystem');
  assert(fs.lstatSync(longExportFile).isFile() && !fs.lstatSync(longExportFile).isSymbolicLink(), 'Long export creates a regular Markdown file');
  const exportLines = fs.readFileSync(longExportFile, 'utf8').split(/\r?\n/);
  const exportedMarkers = exportLines.filter(line => line.startsWith('Synthetic Beta message '));
  assert(exportedMarkers.length === 1005 && exportedMarkers.every((line, index) => line === `Synthetic Beta message ${index + 1}`), 'Real Windows export contains all 1005 messages exactly once in chronological order');
  const exportProbe = await cdp.evaluate(`window.__testCalls.find(c => c.cmd === 'get_messages' && c.args.filePath === ${JSON.stringify(exportBeta.filePath)})`);
  assert(!exportProbe, 'Unified Windows export does not prefetch messages through bounded detail pagination');
  fs.rmSync(longExportFile, { force: true });
  candidateExportFiles.delete(longExportFile);

  const exportArgs = {
    sessionId: exportBeta.id, projectPath: exportBeta.cwd, filePath: exportBeta.filePath,
    provider: 'claude', title: exportBeta.title, updatedAt: 'synthetic concurrent export',
  };
  const concurrentExports = await cdp.evaluate(`(async () => Promise.all([invoke('export_session_markdown', ${JSON.stringify(exportArgs)}), invoke('export_session_markdown', ${JSON.stringify(exportArgs)})]))()`);
  assert(concurrentExports.length === 2 && concurrentExports.every(result => result.message_count === 1005 && !result.cleanup_warning), 'Two real concurrent Windows exports complete without cleanup warnings');
  assert(concurrentExports[0].file_path !== concurrentExports[1].file_path, 'Concurrent Windows exports reserve different final filenames');
  for (const result of concurrentExports) {
    assert(isOwnedExport(result.file_path, exportBeta), 'Concurrent export remains inside the owned fixture output set');
    candidateExportFiles.add(result.file_path);
    const markers = fs.readFileSync(result.file_path, 'utf8').split(/\r?\n/).filter(line => line.startsWith('Synthetic Beta message '));
    assert(markers.length === 1005, 'Concurrent export does not overwrite or truncate another export');
  }
  for (const result of concurrentExports) {
    fs.rmSync(result.file_path, { force: true });
    candidateExportFiles.delete(result.file_path);
  }

  const gamma = config.sessions.find(s => s.title.endsWith('Gamma'));
  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(gamma.id)}] .s-body').click()`);
  await cdp.waitFor(`activeSession?.session_id === ${JSON.stringify(gamma.id)} && chatLoaded`);
  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(gamma.id)}]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await cdp.evaluate(`document.getElementById('ctx-export').click()`);
  await cdp.waitFor(`window.__testCalls.some(c => c.cmd === 'export_session_markdown' && c.args.sessionId === ${JSON.stringify(gamma.id)} && c.result?.message_count === 2)`, 30000);
  const shortExportCall = await cdp.evaluate(`window.__testCalls.find(c => c.cmd === 'export_session_markdown' && c.args.sessionId === ${JSON.stringify(gamma.id)})`);
  const shortExportFile = shortExportCall.result?.file_path;
  if (isOwnedExport(shortExportFile, gamma)) candidateExportFiles.add(shortExportFile);
  assert(isOwnedExport(shortExportFile, gamma) && !shortExportCall.result?.cleanup_warning, 'Short Windows export uses the same backend and cleans its workspace');
  const shortMarkers = fs.readFileSync(shortExportFile, 'utf8').split(/\r?\n/).filter(line => line.startsWith('Synthetic Gamma message '));
  assert(shortMarkers.length === 2 && shortMarkers[0] === 'Synthetic Gamma message 1' && shortMarkers[1] === 'Synthetic Gamma message 2', 'Short backend export contains both messages in chronological order');
  fs.rmSync(shortExportFile, { force: true });
  candidateExportFiles.delete(shortExportFile);
  await cdp.evaluate(`document.getElementById('btn-fav').click()`);
  await cdp.waitFor(`window.__testCalls.some(c => c.cmd === 'toggle_favorite' && c.result === true)`);
  let favorites = JSON.parse(fs.readFileSync(path.join(config.claudeDir, 'favorites.json'), 'utf8'));
  assert(favorites.includes(gamma.id), 'Favorite toggle writes the isolated Windows favorites file');
  await cdp.evaluate(`document.getElementById('btn-fav').click()`);
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'toggle_favorite').length === 2`);
  favorites = JSON.parse(fs.readFileSync(path.join(config.claudeDir, 'favorites.json'), 'utf8'));
  assert(!favorites.includes(gamma.id), 'Second favorite toggle restores the original state');

  await cdp.evaluate(`document.getElementById('provider-tab-codex').click()`);
  await cdp.waitFor(`currentProvider === 'codex' && providerStates.codex.loaded && providerStates.codex.allSessions.length === 2 && providerStates.codex.allSessions.every(s => s.message_count_loaded)`);
  assert(await cdp.evaluate(`providerStates.codex.allSessions.every(s => s.message_count === 1)`), 'Bundled SQLite and rollout parsing load both Codex sessions and counts');
  await cdp.evaluate(`(async () => { window.__cacheRefresh = await invoke('get_sessions', { provider: 'codex' }); return true; })()`);
  assert(await cdp.evaluate(`window.__cacheRefresh.length === 2 && window.__cacheRefresh.every(s => s.message_count_loaded && s.message_count === 1)`), 'Plain Windows database paths and canonical rollout paths share the same populated count cache');
  await cdp.evaluate(`document.querySelector('[data-filter="proj"]').click()`);
  await cdp.waitFor(`document.querySelectorAll('.proj-header').length === 2`);
  const codexProjectMeta = await cdp.evaluate(`[...document.querySelectorAll('.proj-meta')].map(el => el.textContent)`);
  assert(codexProjectMeta.every(text => !text.includes('\\\\?\\')), 'Directory tree strips Codex verbatim Windows prefixes for display');
  const uncSession = config.sessions.find(s => s.provider === 'codex' && s.title.endsWith('Epsilon'));
  assert(await cdp.evaluate(`shortPath(${JSON.stringify(uncSession.cwd)})`) === '\\\\server\\share\\项目 UNC', 'Real WebView2 strips a verbatim UNC prefix while preserving server and share');
  assert(codexProjectMeta.some(text => text.includes('\\\\server\\share')), 'UNC project parent displays server and share in the directory tree');
  await cdp.evaluate(`document.querySelector('[data-filter="all"]').click()`);
  await cdp.waitFor(`document.querySelectorAll('.session-item').length === 2`);
  const x1 = config.sessions.find(s => s.provider === 'codex' && s.favorite);
  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(x1.id)}] .s-body').click()`);
  await cdp.waitFor(`activeSession?.session_id === ${JSON.stringify(x1.id)} && chatLoaded && chatMsgs.length === 2`);
  assert(await cdp.evaluate(`chatMsgs.at(-1).text.includes('**answer**')`), 'Codex detail displays the latest synthetic answer');
  await cdp.screenshot(path.join(config.root, 'windows-codex-detail.png'));
  await cdp.evaluate(`document.getElementById('provider-tab-claude').click()`);
  await cdp.waitFor(`currentProvider === 'claude'`);

  await cdp.evaluate(`document.getElementById('btn-open-favs').click()`);
  batchAttempted = true;
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length === 1`, 40000);
  let call = await cdp.evaluate(`window.__testCalls.find(c => c.cmd === 'open_sessions_terminal')`);
  fs.writeFileSync(path.join(config.root, 'first-batch-result.json'), JSON.stringify(call, null, 2));
  assert(call.args.sessions.length === 2 && call.args.sessions.every(s => s.provider === 'claude'), 'Claude favorites stay in the Claude batch');
  assert(call.result?.opened === 2 && call.result?.already_running === 0, `Claude-only batch opens two newly verified providers: ${JSON.stringify(call)}`);
  const claudeNativeText = runPowerShell('verify-tabs.ps1', 'claude', '', 'claude-batch');
  const claudeNative = JSON.parse(claudeNativeText.trim().split(/\r?\n/).at(-1));
  assert(claudeNative.NativeWindowCount === 1 && claudeNative.TabCount === 2, 'UI Automation verifies one Claude window with exactly two top tabs');

  await cdp.evaluate(`document.getElementById('btn-open-favs').click()`);
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length === 2`, 40000);
  call = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.result?.opened === 0 && call.result?.already_running === 2, 'Repeated Claude batch skips only the two running Claude sessions');
  await cdp.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(c1.id)}] .s-body').click()`);
  await cdp.waitFor(`activeSession?.session_id === ${JSON.stringify(c1.id)} && chatLoaded`);
  await cdp.evaluate(`document.getElementById('open-btn').click()`);
  await cdp.waitFor(`window.__testCalls.some(c => c.cmd === 'open_session')`, 30000);
  call = await cdp.evaluate(`window.__testCalls.find(c => c.cmd === 'open_session')`);
  assert(call.error?.includes('未重复启动'), 'Single-open refuses a running session when native focus is unavailable');

  // Regression: a service-like app-server-daemon may retain a rollout handle
  // after an interactive Terminal closes. It must not block a CLI-origin
  // session, while the same evidence must remain conservative for VS Code.
  const cliDaemonState = startDaemon(x1);
  const vscodeDaemonState = startDaemon(uncSession);
  const [cliDaemon, vscodeDaemon] = await Promise.all([
    waitForDaemon(cliDaemonState), waitForDaemon(vscodeDaemonState),
  ]);
  await cdp.evaluate(`document.getElementById('provider-tab-codex').click()`);
  await cdp.waitFor(`currentProvider === 'codex'`);
  await cdp.evaluate(`document.getElementById('btn-open-favs').click()`);
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length === 3`, 40000);
  call = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').at(-1)`);
  fs.writeFileSync(path.join(config.root, 'sequential-codex-batch-result.json'), JSON.stringify(call, null, 2));
  const vscodeDaemonCall = await cdp.evaluate(`(async () => {
    const session = providerStates.codex.allSessions.find(s => s.session_id === ${JSON.stringify(uncSession.id)});
    try {
      const result = await invoke('open_sessions_terminal', { sessions: [session] });
      return { args: { sessions: [session] }, result };
    } catch (error) { return { args: { sessions: [session] }, error: String(error) }; }
  })()`);
  fs.writeFileSync(path.join(config.root, 'codex-daemon-guard-result.json'), JSON.stringify({
    cliDaemon, vscodeDaemon, cliCall: call, vscodeCall: vscodeDaemonCall,
  }, null, 2));
  assert(call.args.sessions.length === 1 && call.args.sessions[0].provider === 'codex', 'Codex favorite query does not inherit the Claude selection');
  assert(call.result?.opened === 1 && call.result?.already_running === 0, `CLI Codex rollout held only by app-server-daemon can open: ${JSON.stringify(call)}`);
  assert(vscodeDaemonCall.result?.opened === 0 && vscodeDaemonCall.result?.already_running === 1,
    `VS Code Codex rollout held by app-server-daemon stays protected: ${JSON.stringify(vscodeDaemonCall)}`);
  assert(call.args.sessions[0].source === 'cli' && vscodeDaemonCall.args.sessions[0].source === 'vscode',
    'Real backend preserves Codex thread source through the batch IPC');
  const codexNativeText = runPowerShell('verify-tabs.ps1', 'codex', '', 'codex-batch');
  const codexNative = JSON.parse(codexNativeText.trim().split(/\r?\n/).at(-1));
  assert(codexNative.NativeWindowCount === 1 && codexNative.TabCount === 1, 'UI Automation verifies the separately opened Codex window and tab');
  assert(codexNative.Hwnd !== claudeNative.Hwnd, 'Sequential provider batches create distinct owned native windows');

  await cdp.evaluate(`document.getElementById('btn-open-favs').click()`);
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length === 5`, 40000);
  call = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.result?.opened === 0 && call.result?.already_running === 1, 'Only the genuinely running Codex session is skipped on repeat');

  const entries = providerEntries().filter(item => !item.daemon);
  assert(entries.length === 3, 'Exactly three interactive fixture providers were created across five sequential batch attempts');
  for (const session of config.sessions.filter(s => s.favorite)) {
    const entry = entries.find(item => item.provider === session.provider && item.sessionId === session.id);
    assert(!!entry, `${session.provider} fixture started for ${session.title}`);
    const expected = session.provider === 'claude' ? ['--dangerously-skip-permissions', '-r', session.id] : ['--dangerously-bypass-approvals-and-sandbox', 'resume', session.id];
    assert(JSON.stringify(entry.args) === JSON.stringify(expected), `${session.provider} receives exact resume flags`);
    assert(entry.cwd.toLowerCase() === path.resolve(session.cwd).toLowerCase(), `${session.provider} preserves the special-character working directory`);
    assert(entry.environmentInherited && entry.parentMarkersCleared, `${session.provider} inherits the test environment and clears parent markers`);
  }
  // Check invalid cwd while Gamma is still stopped. After the concurrent
  // launch below, the same session must instead be skipped as already running.
  const concurrentBase = await cdp.evaluate(`providerStates.claude.allSessions.find(s => s.session_id === ${JSON.stringify(gamma.id)})`);
  const missingCwd = path.join(config.root, 'missing-project-directory');
  const invalidCwdOutcome = await cdp.evaluate(`(async () => {
    try {
      const result = await invoke('open_sessions_terminal', { sessions: [{ ...${JSON.stringify(concurrentBase)}, project_path: ${JSON.stringify(missingCwd)} }] });
      return { result };
    } catch (error) { return { error: String(error) }; }
  })()`);
  fs.writeFileSync(path.join(config.root, 'invalid-cwd-result.json'), JSON.stringify(invalidCwdOutcome, null, 2));
  assert(invalidCwdOutcome.error?.includes('未回退到 home') && !invalidCwdOutcome.result, 'Invalid non-empty cwd is rejected before any launch without home fallback');

  // Regression: the backend lock is keyed by Provider + session ID, not by the
  // backing path. Direct concurrent IPC bypasses the frontend in-flight guard;
  // aliases must still produce exactly one interactive Provider and native tab.
  const aliasCwd = path.join(config.root, '项目 concurrent alias');
  const aliasFile = path.join(config.claudeDir, 'projects', 'project-concurrent-alias', `${gamma.id}.jsonl`);
  fs.mkdirSync(aliasCwd, { recursive: true });
  fs.mkdirSync(path.dirname(aliasFile), { recursive: true });
  fs.copyFileSync(gamma.filePath, aliasFile);
  const concurrentRequests = Array.from({ length: 8 }, (_, index) => ({
    sessionId: gamma.id,
    projectPath: index % 2 ? aliasCwd : concurrentBase.project_path,
    filePath: index % 2 ? aliasFile : concurrentBase.file_path,
    provider: 'claude',
    source: '',
    title: gamma.title,
  }));
  batchAttempted = true;
  const concurrentOutcomes = await cdp.evaluate(`(async () => Promise.all(${JSON.stringify(concurrentRequests)}.map(async args => {
    try { return { result: await invoke('open_session', args) }; }
    catch (error) { return { error: String(error) }; }
  })))()`);
  const concurrentEntries = providerEntries().filter(item => !item.daemon && item.provider === 'claude' && item.sessionId === gamma.id);
  const concurrentNativeText = runPowerShell('verify-tabs.ps1', 'claude', gamma.id, 'claude-concurrent-alias');
  const concurrentNative = JSON.parse(concurrentNativeText.trim().split(/\r?\n/).at(-1));
  fs.writeFileSync(path.join(config.root, 'concurrent-alias-open-result.json'), JSON.stringify({
    originalFile: concurrentBase.file_path, aliasFile, outcomes: concurrentOutcomes,
    providers: concurrentEntries, nativeWindow: concurrentNative,
  }, null, 2));
  const concurrentOpened = concurrentOutcomes.filter(outcome => outcome.result === 'opened').length;
  const concurrentDuplicates = concurrentOutcomes.filter(outcome => outcome.result === 'focused' || outcome.error?.includes('未重复启动')).length;
  assert(concurrentOutcomes.length === 8 && concurrentOpened === 1 && concurrentDuplicates === 7,
    `Same UUID aliases serialize behind one real launch: ${JSON.stringify(concurrentOutcomes)}`);
  assert(concurrentEntries.length === 1, 'Concurrent alias IPC creates exactly one interactive Provider');
  assert(concurrentNative.NativeWindowCount === 1 && concurrentNative.TabCount === 1,
    'Concurrent alias IPC creates exactly one owned native tab');
  assert(concurrentNative.Hwnd !== claudeNative.Hwnd,
    'The disjoint fresh session forms a new window instead of joining or replacing an existing batch');

  for (const failureSession of config.failureSessions) {
    fs.mkdirSync(path.dirname(failureSession.filePath), { recursive: true });
    fs.writeFileSync(failureSession.filePath, [
      { type: 'custom-title', customTitle: failureSession.title, timestamp: new Date().toISOString(), cwd: failureSession.cwd },
      { type: 'user', timestamp: new Date().toISOString(), cwd: failureSession.cwd, message: { content: 'Synthetic negative launch fixture' } },
    ].map(line => JSON.stringify(line)).join('\n') + '\n');
    const launch = {
      session_id: failureSession.id, provider: 'claude', session_kind: 'main', project_slug: 'project-b',
      project_path: failureSession.cwd, title: failureSession.title, message_count: 1,
      message_count_loaded: true, updated_at: 'synthetic', updated_at_ts: 1,
      created_at: 'synthetic', created_at_ts: 1, file_path: failureSession.filePath, favorited: false,
    };
    const outcome = await cdp.evaluate(`(async () => {
      try {
        const result = await invoke('open_sessions_terminal', { sessions: [${JSON.stringify(launch)}] });
        return { result };
      } catch (error) { return { error: String(error) }; }
    })()`);
    assert(outcome.error?.includes('0/1') && outcome.error.includes('未执行自动清理') && !outcome.result,
      `${failureSession.launchBehavior} does not report opened without a new provider process`);
  }
  const entriesAfterFailures = providerEntries().filter(item => !item.daemon);
  assert(entriesAfterFailures.length === 4, 'Immediate-exit and unconfirmed-Trust fixtures create no additional interactive provider process');

  // Regression: closing the owned Codex tab must not leave the session blocked
  // forever. Reopen only after the exact tab close, then require one new provider.
  const codexBeforeClose = entriesAfterFailures.find(item => !item.daemon && item.provider === 'codex' && item.sessionId === x1.id);
  assert(codexBeforeClose, 'Owned Codex provider identity exists before tab close');
  execFileSync('powershell.exe', [
    '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(here, 'close-tabs.ps1'),
    '-ConfigPath', configPath, '-SessionId', x1.id,
  ], { encoding: 'utf8', timeout: 45000, windowsHide: true });
  const providerAlive = pid => {
    try { process.kill(pid, 0); return true; } catch (error) { return error.code === 'EPERM'; }
  };
  let providerAliveAfterClose = true;
  let providerExitWaitMs = 0;
  for (; providerExitWaitMs <= 10000; providerExitWaitMs += 250) {
    if (!providerAlive(codexBeforeClose.pid)) { providerAliveAfterClose = false; break; }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  const callsBeforeReopen = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length`);
  await cdp.evaluate(`document.getElementById('btn-open-favs').click()`);
  await cdp.waitFor(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').length === ${callsBeforeReopen + 1}`, 40000);
  const reopenCall = await cdp.evaluate(`window.__testCalls.filter(c => c.cmd === 'open_sessions_terminal').at(-1)`);
  let reopenedNative = null;
  let reopenedNativeError = null;
  try {
    const reopenedNativeText = runPowerShell('verify-tabs.ps1', 'codex', '', 'codex-reopen');
    reopenedNative = JSON.parse(reopenedNativeText.trim().split(/\r?\n/).at(-1));
  } catch (error) { reopenedNativeError = String(error); }
  const entriesAfterReopen = providerEntries();
  const reopenedProvider = entriesAfterReopen.filter(item => !item.daemon && item.provider === 'codex' && item.sessionId === x1.id).at(-1);
  fs.writeFileSync(path.join(config.root, 'codex-reopen-result.json'), JSON.stringify({
    beforeClose: codexBeforeClose,
    providerAliveAfterClose,
    providerExitWaitMs,
    reopenCall,
    reopenedProvider,
    nativeWindow: reopenedNative,
    nativeWindowError: reopenedNativeError,
  }, null, 2));
  assert(reopenCall.result?.opened === 1 && reopenCall.result?.already_running === 0,
    `Closed Codex tab can be opened again: ${JSON.stringify({ providerAliveAfterClose, providerExitWaitMs, result: reopenCall.result, error: reopenCall.error })}`);
  assert(reopenedNative?.NativeWindowCount === 1 && reopenedNative?.TabCount === 1, `Reopened Codex has one owned native tab: ${reopenedNativeError || 'UIA available'}`);
  assert(entriesAfterReopen.filter(item => !item.daemon).length === 5 && reopenedProvider && reopenedProvider.pid !== codexBeforeClose.pid,
    'Closed Codex reopen creates exactly one new interactive provider process');

  fs.writeFileSync(path.join(config.root, 'network-requests.json'), JSON.stringify(cdp.requests, null, 2));
  assert(cdp.errors.length === 0, 'Real Windows WebView2 reports no JavaScript exceptions');
  console.log(`WINDOWS_GUI_SMOKE_OK run=${config.runId} nativeWindow=3 nativeTabs=4 providers=5 sequence=claude_then_codex negativeLaunchGuards=3 codexReopen=1 daemonSourceGuards=2 concurrentAliasRequests=8 concurrentAliasOpened=1`);
} catch (error) {
  failure = error;
} finally {
  if (cdp) {
    try {
      const exportFiles = await cdp.evaluate(`window.__testCalls?.filter(c => c.cmd === 'export_session_markdown').map(c => c.result?.file_path).filter(Boolean) || []`);
      for (const file of exportFiles) {
        if (isOwnedExport(file)) candidateExportFiles.add(file);
      }
    } catch (error) { failure ||= error; console.error(error); }
    try { await cdp.close(); } catch (error) { failure ||= error; console.error(error); }
  }
  for (const file of candidateExportFiles) {
    if (isOwnedExport(file)) {
      try { fs.rmSync(file, { force: true }); } catch (error) { failure ||= error; console.error(error); }
    }
  }
  if (batchAttempted) {
    try { console.log(runPowerShell('close-tabs.ps1').trim()); } catch (error) { failure ||= error; console.error(error); }
  }
  try { console.log(runPowerShell('cleanup-providers.ps1').trim()); } catch (error) { failure ||= error; console.error(error); }
  if (appSpawnError) failure ||= appSpawnError;
  if (!appExited() && !appSpawnError) {
    appTerminationRequested = true;
    try {
      if (!app.kill()) failure ||= new Error(`Test app ${app.pid} could not be signaled for cleanup`);
    } catch (error) { failure ||= error; console.error(error); }
    const exited = await waitForAppExit(5000);
    if (!exited) failure ||= new Error(`Test app ${app.pid} is still running 5 seconds after cleanup`);
  }
  if (!appTerminationRequested && app.exitCode !== null && app.exitCode !== 0) {
    failure ||= new Error(`Test app exited unexpectedly with code ${app.exitCode}`);
  }
  fs.writeFileSync(path.join(config.root, 'gui-smoke-result.json'), JSON.stringify({ ok: !failure, error: failure ? String(failure) : null, finishedAt: new Date().toISOString() }, null, 2));
}
if (failure) throw failure;
