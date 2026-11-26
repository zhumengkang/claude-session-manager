import { connectCdp, assert } from './cdp.mjs';

const ids = {
  c1: '00000000-0000-4000-8000-0000000000c1',
  c2: '00000000-0000-4000-8000-0000000000c2',
  c3: '00000000-0000-4000-8000-0000000000c3',
  x1: '00000000-0000-4000-8000-0000000000a1',
  x2: '00000000-0000-4000-8000-0000000000a2',
  x3: '00000000-0000-4000-8000-0000000000a3',
  x4: '00000000-0000-4000-8000-0000000000a4',
};
function session(id, provider, title, project, kind = 'main') {
  return {
    session_id: id, provider, session_kind: kind,
    project_slug: project.replaceAll('/', '-'), project_path: project,
    title, message_count: 5, message_count_loaded: true, updated_at: '刚才', updated_at_ts: 100,
    created_at: '今天', created_at_ts: 90,
    file_path: `/mock/${id}.jsonl`, favorited: false,
  };
}
const sessions = [
  session(ids.c1, 'claude', 'Alpha Claude', '/mock/alpha'),
  session(ids.c2, 'claude', 'Beta Claude', '/mock/beta'),
  session(ids.c3, 'claude', 'Gamma Claude', '/mock/alpha'),
  session(ids.x1, 'codex', 'Codex Main One', '/mock/codex-a'),
  session(ids.x2, 'codex', 'Codex Main Two', '/mock/codex-b'),
  session(ids.x3, 'codex', 'Codex Child One', '/mock/codex-a', 'subagent'),
  session(ids.x4, 'codex', 'Codex Child Two', '/mock/codex-b', 'subagent'),
];
sessions[3].source = 'cli';
sessions[5].message_count = 0;
sessions[5].message_count_loaded = false;
const messages = {};
for (const s of sessions) {
  messages[s.file_path] = [
    { role: 'user', text: `${s.title} user question`, timestamp: '11/23 04:00' },
    { role: 'assistant', text: `${s.title} answer`, timestamp: '11/23 04:01' },
  ];
}
messages[sessions[0].file_path] = Array.from({ length: 205 }, (_, i) => ({
  role: i % 2 ? 'assistant' : 'user',
  text: i === 203
    ? 'Code sample:\n```js\nconst answer = 42;\n```'
    : i === 204 ? 'Claude message-205 **latest**' : `Claude message-${i + 1}`,
  timestamp: `11/23 04:${String(i % 60).padStart(2, '0')}`,
}));
messages[sessions[3].file_path] = [
  { role: 'user', text: 'Codex oldest question', timestamp: '11/23 03:00' },
  { role: 'assistant', text: '```python\nprint("ok")\n```', timestamp: '11/23 03:01' },
  { role: 'assistant', text: 'Codex latest **bold** <img src=x onerror=window.__xss=1>', timestamp: '11/23 03:02' },
];

messages[sessions[4].file_path] = Array.from({ length: 1005 }, (_, i) => ({
  role: i % 2 ? 'assistant' : 'user',
  text: `Codex long message-${i + 1}`,
  timestamp: `11/23 05:${String(i % 60).padStart(2, '0')}`,
}));

const mockSource = `
(() => {
  const sessions = ${JSON.stringify(sessions)};
  const messages = ${JSON.stringify(messages)};
  const key = (provider, id) => provider + ':' + id;
  const state = {
    sessions,
    messages,
    calls: [],
    clipboard: [],
    favorites: new Set([key('claude', '${ids.c1}'), key('codex', '${ids.x2}')]),
    failCountsOnce: new Set(),
    failMessagesOnce: new Set(),
    blockFavoriteFetch: false,
    favoriteFetchResolvers: [],
    autostart: false,
    failAutostartOnce: false,
    updateSettings: { current_version: '0.1.0', github_repository: '', auto_check: true, dismissed_version: null },
    updateResult: null,
    updateCheckError: '',
    openedUpdateUrls: [],
  };
  window.__mock = state;
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
    writeText: async text => { state.clipboard.push(String(text)); }
  }});
  window.__TAURI__ = { core: { invoke: async (cmd, args = {}) => {
    state.calls.push({ cmd, args: JSON.parse(JSON.stringify(args)) });
    if (cmd === 'get_sessions') {
      return sessions.filter(s => s.provider === args.provider).map(s => ({
        ...s, favorited: state.favorites.has(key(s.provider, s.session_id))
      }));
    }
    if (cmd === 'get_favorite_sessions') {
      if (state.blockFavoriteFetch) {
        await new Promise(resolve => state.favoriteFetchResolvers.push(resolve));
      }
      return sessions.filter(s => s.provider === args.provider && state.favorites.has(key(s.provider, s.session_id)))
        .map(s => ({ ...s, favorited: true }));
    }
    if (cmd === 'get_codex_message_count') {
      if (state.failCountsOnce.delete(args.filePath)) throw new Error('Synthetic transient count failure');
      if (args.filePath === '/mock/${ids.x3}.jsonl') return 7;
      if (args.filePath === '/mock/${ids.x4}.jsonl') return 9;
      throw new Error('Unexpected Codex count path');
    }
    if (cmd === 'get_claude_storage_stats') {
      return { recoverable: 3, history_ids: 10, history_only: 7, subagent_records: 4 };
    }
    if (cmd === 'get_messages') {
      const owner = sessions.find(s => s.file_path === args.filePath && s.provider === args.provider);
      if (!owner || args.sessionId !== owner.session_id || args.projectPath !== owner.project_path) {
        throw new Error('Message page identity validation failed');
      }
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(args.sessionId)) {
        throw new Error('Message page session ID is not a UUID');
      }
      if (state.failMessagesOnce.delete(args.filePath)) {
        throw new Error('Synthetic transient message-page failure');
      }
      const all = messages[args.filePath] || [];
      const limit = Math.min(args.limit || 200, 1000);
      const offset = args.offset || 0;
      const snapshotLen = args.snapshotLen ?? all.length;
      if (!Number.isInteger(snapshotLen) || snapshotLen < 0 || snapshotLen > all.length) {
        throw new Error('Message page snapshot is invalid or truncated');
      }
      const end = Math.max(0, snapshotLen - offset);
      const start = Math.max(0, end - limit);
      const page = all.slice(start, end);
      const hasMore = start > 0;
      const next = offset + page.length;
      return {
        messages: page,
        total: hasMore ? next + 1 : snapshotLen,
        offset: next,
        next_offset: next,
        total_exact: !hasMore,
        has_more: hasMore,
        snapshot_len: snapshotLen,
        snapshot_token: args.snapshotToken || ('snapshot:' + args.filePath),
      };
    }
    if (cmd === 'export_session_markdown') {
      const all = messages[args.filePath] || [];
      return {
        file_path: '/mock/exports/' + args.sessionId + '.md',
        message_count: all.length,
        cleanup_warning: window.__mockExportCleanupWarning || undefined,
      };
    }
    if (cmd === 'toggle_favorite') {
      const k = key(args.provider, args.sessionId);
      if (state.favorites.has(k)) { state.favorites.delete(k); return false; }
      state.favorites.add(k); return true;
    }
    if (cmd === 'open_session') return 'opened';
    if (cmd === 'open_sessions_terminal') {
      return { opened: args.sessions.length, already_running: 0 };
    }
    if (cmd === 'should_open_favorites') return false;
    if (cmd === 'get_autostart_status') return state.autostart;
    if (cmd === 'setup_autostart') {
      if (state.failAutostartOnce) {
        state.failAutostartOnce = false;
        throw new Error('Synthetic autostart failure');
      }
      state.autostart = true;
      return '已设置开机自启';
    }
    if (cmd === 'remove_autostart') {
      state.autostart = false;
      return '已移除开机自启';
    }
    if (cmd === 'get_update_settings') return { ...state.updateSettings };
    if (cmd === 'save_update_settings') {
      const repository = String(args.githubRepository || '').trim();
      if (repository && !/^[A-Za-z0-9_.-]+\\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error('Invalid synthetic repository');
      if (state.updateSettings.github_repository !== repository) state.updateSettings.dismissed_version = null;
      state.updateSettings.github_repository = repository;
      state.updateSettings.auto_check = args.autoCheck === true;
      return { ...state.updateSettings };
    }
    if (cmd === 'check_for_updates') {
      if (state.updateCheckError) throw new Error(state.updateCheckError);
      if (!state.updateSettings.github_repository) throw new Error('Synthetic missing repository');
      if (!state.updateResult) throw new Error('Synthetic update result not configured');
      return {
        ...state.updateResult,
        current_version: state.updateSettings.current_version,
        repository: state.updateSettings.github_repository,
        dismissed: state.updateSettings.dismissed_version === state.updateResult.latest_version,
      };
    }
    if (cmd === 'dismiss_update') {
      state.updateSettings.dismissed_version = args.version;
      return { ...state.updateSettings };
    }
    if (cmd === 'open_update_release') {
      state.openedUpdateUrls.push(args.releaseUrl);
      return null;
    }
    if (cmd === 'reveal_in_finder') return null;
    throw new Error('Unexpected mock command: ' + cmd);
  }}};
})();`;

const c = await connectCdp(Number(process.env.CSM_CDP_PORT || 9223));
const pageUrl = new URL('../src/index.html', import.meta.url).href;
const click = selector => c.evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const sessionClick = id => c.evaluate(`document.querySelector('.session-item[data-id=${JSON.stringify(id)}] .s-body').click()`);
const callCount = cmd => c.evaluate(`window.__mock.calls.filter(call => call.cmd === ${JSON.stringify(cmd)}).length`);
try {
  await c.send('Page.addScriptToEvaluateOnNewDocument', { source: mockSource });
  await c.navigate(pageUrl);
  await c.waitFor(`window.__mock && providerStates.claude.loaded && document.querySelectorAll('.session-item').length === 3`);
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 3, 'Claude initial list renders 3 sessions');
  assert((await c.evaluate(`document.getElementById('storage-note').textContent`)).includes('3 可恢复'), 'Claude storage statistics are visible');
  assert((await c.evaluate(`document.getElementById('storage-note').textContent`)).includes('7 历史ID无文件'), 'History-only explanation is visible');
  assert(c.requests.some(url => url.includes('vendor/tabler/tabler.min.css')), 'Tabler CSS is actually loaded');
  const windowsProject = 'C:\\work\\项目';
  const windowsHomeProject = 'C:\\Users\\someone\\work';
  const windowsVerbatimProject = '\\\\?\\C:\\Users\\someone\\work';
  const windowsUncVerbatimProject = '\\\\?\\UNC\\server\\share\\项目 UNC';
  assert(await c.evaluate(`pathBaseName(${JSON.stringify(windowsProject)})`) === '项目', 'Windows project basename handles backslashes and Unicode');
  assert(await c.evaluate(`parentProjectPath(${JSON.stringify(windowsProject)})`) === 'C:\\work', 'Windows project parent preserves the backslash path');
  assert(await c.evaluate(`shortPath(${JSON.stringify(windowsHomeProject)})`) === '~\\work', 'Windows home paths are shortened without a hard-coded username');
  assert(await c.evaluate(`shortPath(${JSON.stringify(windowsVerbatimProject)})`) === '~\\work', 'Codex verbatim Windows paths are normalized for display');
  assert(await c.evaluate(`shortPath(${JSON.stringify(windowsUncVerbatimProject)})`) === '\\\\server\\share\\项目 UNC', 'UNC verbatim paths lose only the verbatim prefix and retain server/share identity');
  assert(await c.evaluate(`pathBaseName(shortPath(${JSON.stringify(windowsUncVerbatimProject)}))`) === '项目 UNC', 'UNC project tree displays the actual folder name');
  assert(await c.evaluate(`parentProjectPath(shortPath(${JSON.stringify(windowsUncVerbatimProject)}))`) === '\\\\server\\share', 'UNC project parent retains server and share');
  await c.screenshot('/tmp/csm-ui-claude.png');

  await click('[data-filter="main"]');
  await c.waitFor(`filteredList.length === 3`);
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 3, 'Claude main filter preserves all main sessions');
  await click('[data-filter="recent"]');
  await c.waitFor(`filteredList.length === 3`);
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 3, 'Recent-30 filter works');
  await c.evaluate(`(() => {
    window.__filterFixture = { allSessions, filteredList, currentFilter, sortOrder };
    allSessions = Array.from({ length: 35 }, (_, i) => ({
      session_id: 'recent-' + i, provider: 'claude', session_kind: 'main',
      title: 'Session ' + String(i + 1).padStart(2, '0'), updated_at_ts: i + 1,
      project_path: '/mock/recent', message_count_loaded: true, message_count: 1,
    }));
    currentFilter = 'recent'; sortOrder = 'name'; applyFilter();
    window.__recentSnapshot = filteredList.map(s => ({ title: s.title, updated_at_ts: s.updated_at_ts }));
    const saved = window.__filterFixture;
    allSessions = saved.allSessions; filteredList = saved.filteredList;
    currentFilter = saved.currentFilter; sortOrder = saved.sortOrder; applyFilter();
    delete window.__filterFixture;
    return true;
  })()`);
  const recentSnapshot = await c.evaluate(`window.__recentSnapshot`);
  assert(recentSnapshot.length === 30 && recentSnapshot.every((s, index) => s.updated_at_ts === index + 6), 'Recent-30 selects the newest 30 before applying name display sorting');
  await c.evaluate(`delete window.__recentSnapshot; true`);
  await click('[data-filter="fav"]');
  await c.waitFor(`filteredList.length === 1`);
  assert(await c.evaluate(`filteredList[0].session_id`) === ids.c1, 'Favorites filter shows the expected Claude session');
  await click('[data-filter="all"]');
  await c.waitFor(`filteredList.length === 3`);

  await c.evaluate(`document.getElementById('search').value = 'Beta'; document.getElementById('search').dispatchEvent(new Event('input', {bubbles:true}))`);
  await c.waitFor(`filteredList.length === 1`);
  assert(await c.evaluate(`filteredList[0].title`) === 'Beta Claude', 'Search filters by title');
  await click('#provider-tab-codex');
  await c.waitFor(`currentProvider === 'codex' && providerStates.codex.loaded`);
  assert(await c.evaluate(`document.getElementById('search').value`) === '', 'Codex has independent search state');
  await click('#provider-tab-claude');
  await c.waitFor(`currentProvider === 'claude'`);
  assert(await c.evaluate(`document.getElementById('search').value`) === 'Beta', 'Claude search state survives provider switching');
  assert(await c.evaluate(`filteredList.length`) === 1, 'Claude filtered result survives provider switching');
  await c.evaluate(`document.getElementById('search').value = ''; document.getElementById('search').dispatchEvent(new Event('input', {bubbles:true}))`);
  await c.waitFor(`filteredList.length === 3`);

  await click('[data-filter="proj"]');
  await c.waitFor(`document.querySelectorAll('.proj-header').length === 2`);
  assert(await c.evaluate(`document.querySelectorAll('.proj-header').length`) === 2, 'Directory grouping combines sessions by project');
  await c.evaluate(`document.querySelector('.proj-header').click()`);
  await c.waitFor(`document.querySelectorAll('.session-item.proj-item').length > 0`);
  await c.evaluate(`document.querySelector('.proj-selall').click()`);
  assert(await c.evaluate(`selectedIds.size`) === 2, 'Project select-all selects every session in that directory');
  await click('[data-filter="all"]');
  await c.evaluate(`document.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true}))`);
  await c.waitFor(`selectedIds.size === 0`);

  await sessionClick(ids.c1);
  await c.waitFor(`activeSession?.session_id === '${ids.c1}' && chatLoaded && chatMsgs.length === 200`);
  assert(await c.evaluate(`chatMsgs[0].text`) === 'Claude message-6', 'Initial detail page contains the latest 200 messages');
  assert((await c.evaluate(`chatMsgs.at(-1).text`)).includes('message-205'), 'Newest message is at the bottom of the initial page');
  assert(await c.evaluate(`chatOffset`) === 200 && await c.evaluate(`chatHasMore`), 'Cursor points to the next older page');
  assert(!(await c.evaluate(`chatTotalExact`)), 'Unknown total is not marked exact');
  assert((await c.evaluate(`document.getElementById('load-more-btn').textContent`)).includes('已加载 200 条'), 'Load-more label uses loaded count for unknown total');
  assert(await c.evaluate(`!!document.querySelector('#chat-area pre')`), 'Markdown code blocks render as code elements');
  assert(await c.evaluate(`!!document.querySelector('#chat-area .msg-body strong')`), 'Markdown bold text renders correctly');
  assert(await c.evaluate(`chatSnapshotLen`) === 205, 'Initial detail captures the backend snapshot length');
  assert(await c.evaluate(`chatSnapshotToken`) === ('snapshot:' + sessions[0].file_path), 'Initial detail captures the backend snapshot token');
  await c.screenshot('/tmp/csm-ui-detail.png');
  await c.evaluate(`window.__mock.messages[${JSON.stringify(sessions[0].file_path)}].push({ role: 'assistant', text: 'Appended after initial snapshot', timestamp: '11/23 05:00' }); true`);
  await click('#load-more-btn');
  await c.waitFor(`chatMsgs.length === 205 && !chatHasMore`);
  const olderPageCall = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages' && call.args.filePath === ${JSON.stringify(sessions[0].file_path)}).at(-1)`);
  assert(olderPageCall.args.snapshotLen === 205, 'Older-page request reuses the initial fixed snapshot after the source grows');
  assert(olderPageCall.args.snapshotToken === ('snapshot:' + sessions[0].file_path), 'Older-page request reuses the initial snapshot token');
  assert(await c.evaluate(`chatMsgs[0].text`) === 'Claude message-1', 'Loading older messages prepends the missing oldest page');
  assert(await c.evaluate(`new Set(chatMsgs.map(m => m.text)).size`) === 205, 'Appending to the source cannot duplicate messages across fixed-snapshot pages');
  assert(!(await c.evaluate(`chatMsgs.some(m => m.text === 'Appended after initial snapshot')`)), 'A later append is excluded until the detail is reopened');
  assert(await c.evaluate(`chatOffset`) === 205 && await c.evaluate(`chatTotalExact`), 'Final page has an exact exhausted cursor and total');

  await click('#open-btn');
  await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'open_session')`);
  let call = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'open_session')`);
  assert(call.args.provider === 'claude' && call.args.sessionId === ids.c1 && call.args.source === '', 'Single-open passes Claude provider, session ID and empty source');
  assert(call.args.title === 'Alpha Claude', 'Single-open passes the visible Claude title for native tab ownership and display');

  await sessionClick(ids.c2);
  await c.waitFor(`activeSession?.session_id === '${ids.c2}' && chatLoaded`);
  await click('#btn-fav');
  await c.waitFor(`window.__mock.favorites.has('claude:${ids.c2}')`);
  assert(await c.evaluate(`window.__mock.favorites.has('claude:${ids.c2}')`), 'Favorite toggle persists through mocked IPC');
  await click('#btn-fav');
  await c.waitFor(`!window.__mock.favorites.has('claude:${ids.c2}')`);
  assert(!(await c.evaluate(`window.__mock.favorites.has('claude:${ids.c2}')`)), 'Unfavorite restores the original state');

  if (!(await c.evaluate(`filteredList.length > 0 && filteredList.every(s => selectedIds.has(sessionKey(s)))`))) await click('#btn-selall');
  assert(await c.evaluate(`selectedIds.size`) === 3, 'Select-all selects all visible Claude sessions');
  assert(!(await c.evaluate(`document.getElementById('btn-terminal').disabled`)), 'Batch single-window multi-tab button is enabled');
  assert(!(await c.evaluate(`document.getElementById('btn-open-favs').disabled`)), 'Favorite batch-open button is enabled');
  assert(await callCount('get_favorite_sessions') === 0, 'Startup does not load favorites for automatic recovery');
  await c.screenshot('/tmp/csm-ui-selection.png');
  await click('#btn-terminal');
  await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'open_sessions_terminal')`);
  call = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'open_sessions_terminal')`);
  assert(call.args.sessions.length === 3 && call.args.sessions.every(s => s.provider === 'claude'), 'Selected Claude sessions are sent in one batch-open request');
  await click('#btn-open-favs');
  await c.waitFor(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length === 2`);
  call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.args.sessions.length === 1 && call.args.sessions.every(s => s.provider === 'claude'), 'Claude favorite batch contains only Claude sessions');
  const claudeFavoritesCall = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_favorite_sessions').at(-1)`);
  assert(claudeFavoritesCall.args.provider === 'claude', 'Claude favorite query is explicitly provider-scoped');

  await click('#provider-tab-codex');
  await c.waitFor(`currentProvider === 'codex' && providerStates.codex.loaded && document.querySelectorAll('.session-item').length === 4 && providerStates.codex.allSessions.every(s => s.message_count_loaded)`);
  await click('#btn-open-favs');
  await c.waitFor(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length === 3`);
  call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.args.sessions.length === 1 && call.args.sessions.every(s => s.provider === 'codex'), 'After Claude opens, Codex favorites still launch in their own new batch');
  const codexFavoritesCall = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_favorite_sessions').at(-1)`);
  assert(codexFavoritesCall.args.provider === 'codex', 'Codex favorite query is explicitly provider-scoped');
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 4, 'Codex list renders all main and child threads');
  assert(await c.evaluate(`document.querySelectorAll('.s-count').length`) === 4, 'Every Codex row has a visible message-count badge');
  assert(await c.evaluate(`providerStates.codex.allSessions.find(s => s.session_id === '${ids.x3}').message_count`) === 7, 'Pending Codex counts are filled asynchronously without blocking the list');
  assert(await c.evaluate(`[...document.querySelectorAll('.s-count')].every(el => ['5','7'].includes(el.textContent))`), 'Codex message counts use real backend values');
  const note = await c.evaluate(`document.getElementById('storage-note').textContent`);
  assert(note.includes('4 线程') && note.includes('2 主会话') && note.includes('2 子线程'), 'Codex thread statistics are correct');
  assert(await c.evaluate(`document.querySelectorAll('.s-kind').length`) === 2, 'Codex child threads have visible badges');
  await click('[data-filter="main"]');
  await c.waitFor(`filteredList.length === 2`);
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 2, 'Codex main-only filter hides child threads');
  assert(await c.evaluate(`document.querySelectorAll('.s-kind').length`) === 0, 'Main-only list contains no child badges');

  await sessionClick(ids.x1);
  await c.waitFor(`activeSession?.session_id === '${ids.x1}' && chatLoaded && chatMsgs.length === 3`);
  assert(await c.evaluate(`chatMsgs[0].text`) === 'Codex oldest question', 'Codex detail messages retain chronological order');
  assert((await c.evaluate(`chatMsgs.at(-1).text`)).includes('Codex latest'), 'Codex detail opens the latest message');
  assert(await c.evaluate(`!!document.querySelector('#chat-area pre')`), 'Codex Markdown code block renders');
  assert(await c.evaluate(`!!document.querySelector('#chat-area .msg-body strong')`), 'Codex Markdown bold renders');
  assert(await c.evaluate(`document.querySelector('#chat-area img') === null && !window.__xss`), 'Session HTML is escaped and does not execute');
  assert((await c.evaluate(`document.getElementById('chat-area').textContent`)).includes('Codex'), 'Codex role label is visible');
  await click('#open-btn');
  await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'open_session' && call.args.provider === 'codex')`);
  call = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'open_session' && call.args.provider === 'codex')`);
  assert(call.args.sessionId === ids.x1 && call.args.source === 'cli', 'Codex single-open passes the correct session ID and thread source');
  assert(call.args.title === 'Codex Main One', 'Codex single-open passes the visible thread title');
  const previousContextOpens = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_session' && call.args.provider === 'codex').length`);
  await c.evaluate(`document.querySelector('.session-item[data-id="${ids.x1}"]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await c.screenshot('/tmp/csm-ui-context-menu.png');
  await click('#ctx-terminal');
  await c.waitFor(`window.__mock.calls.filter(call => call.cmd === 'open_session' && call.args.provider === 'codex').length > ${previousContextOpens}`);
  call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_session' && call.args.provider === 'codex').at(-1)`);
  assert(call.args.sessionId === ids.x1, 'Context menu uses only the single-session open path');

  if (!(await c.evaluate(`filteredList.every(s => selectedIds.has(sessionKey(s)))`))) await click('#btn-selall');
  assert(await c.evaluate(`selectedIds.size`) === 2, 'Codex select-all respects the main-only filter');
  await click('#btn-terminal');
  await c.waitFor(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length === 4`);
  call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.args.sessions.length === 2 && call.args.sessions.every(s => s.provider === 'codex'), 'Selected Codex sessions use the same single-window multi-tab action');
  assert(await callCount('should_open_favorites') === 0, 'Startup never requests automatic favorite recovery');
  assert(await callCount('get_favorite_sessions') === 2, 'Each explicit favorite batch-open queries only its active provider');

  await c.evaluate(`(() => {
    const backend = window.__mock.sessions.find(s => s.session_id === '${ids.x4}');
    backend.message_count_loaded = false;
    window.__mock.failCountsOnce.add(backend.file_path);
    return true;
  })()`);
  await c.evaluate(`(async () => { await loadSessions('codex'); return true; })()`);
  await c.waitFor(`providerStates.codex.allSessions.find(s => s.session_id === '${ids.x4}')?.message_count_error === true`);
  assert(await c.evaluate(`providerStates.codex.allSessions.find(s => s.session_id === '${ids.x4}').message_count_loaded`), 'Transient Codex count failure is rendered without blocking other sessions');
  await c.evaluate(`(async () => { await loadSessions('codex'); return true; })()`);
  await c.waitFor(`providerStates.codex.allSessions.find(s => s.session_id === '${ids.x4}')?.message_count === 9 && !providerStates.codex.allSessions.find(s => s.session_id === '${ids.x4}').message_count_error`);
  assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_codex_message_count' && call.args.filePath === '/mock/${ids.x4}.jsonl').length`) === 2, 'A later refresh retries a previously failed Codex message count');

  await click('[data-filter="fav"]');
  await c.waitFor(`currentProvider === 'codex' && filteredList.length === 1`);
  assert(await c.evaluate(`selectedIds.size === 1 && selectedIds.has(sessionKey(allSessions.find(s => s.session_id === '${ids.x2}')))`), 'Changing filters prunes selections against the new visible list, not the old one');
  const beforeFilteredBatch = await callCount('open_sessions_terminal');
  await click('#btn-terminal');
  await c.waitFor(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length === ${beforeFilteredBatch + 1}`);
  call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').at(-1)`);
  assert(call.args.sessions.length === 1 && call.args.sessions[0].session_id === ids.x2, 'Filtered batch launch excludes a previously selected but now hidden session');
  await click('[data-filter="main"]');
  await c.waitFor(`filteredList.length === 2`);

  await c.evaluate(`document.querySelector('.session-item[data-id="${ids.x1}"]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await click('#ctx-export');
  await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'export_session_markdown' && call.args.sessionId === '${ids.x1}') && document.getElementById('toast').textContent.includes('3 条')`);
  call = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'export_session_markdown' && call.args.sessionId === '${ids.x1}')`);
  assert(call.args.provider === 'codex' && call.args.filePath === sessions[3].file_path && call.args.projectPath === '/mock/codex-a', 'Short export uses the same validated backend path as long export');
  assert((await c.evaluate(`document.getElementById('toast').textContent`)).includes('/mock/exports/'), 'Short export displays the confirmed backend destination instead of assuming a browser download');

  await c.evaluate(`document.querySelector('.session-item[data-id="${ids.x2}"]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await click('#ctx-export');
  await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'export_session_markdown' && call.args.sessionId === '${ids.x2}') && document.getElementById('toast').textContent.includes('1005')`);
  call = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'export_session_markdown' && call.args.sessionId === '${ids.x2}')`);
  assert(call.args.sessionId === ids.x2 && call.args.provider === 'codex', 'Long export passes the selected Codex identity and provider');
  assert(call.args.filePath === sessions[4].file_path && call.args.projectPath === '/mock/codex-b', 'Long export passes source and project paths');
  assert(call.args.title === 'Codex Main Two' && call.args.updatedAt === '刚才', 'Long export passes display metadata');
  const exportProbe = await c.evaluate(`window.__mock.calls.some(call => call.cmd === 'get_messages' && call.args.limit === 1000)`);
  assert(!exportProbe, 'Unified export does not prefetch or truncate messages through the detail-page IPC');
  assert((await c.evaluate(`document.getElementById('toast').textContent`)).includes('/mock/exports/'), 'Long export shows the actual backend destination');

  await c.evaluate(`window.__mockExportCleanupWarning = 'Synthetic cleanup warning'; true`);
  await c.evaluate(`document.querySelector('.session-item[data-id="${ids.x2}"]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await click('#ctx-export');
  await c.waitFor(`document.getElementById('toast').textContent.includes('Synthetic cleanup warning')`);
  assert((await c.evaluate(`document.getElementById('toast').className`)).includes('err'), 'Cleanup warnings are shown as warnings instead of unconditional export success');
  await c.evaluate(`window.__mockExportCleanupWarning = ''; true`);

  await c.evaluate(`document.querySelector('.session-item[data-id="${ids.x1}"]').dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, clientX:200, clientY:200}))`);
  await click('#ctx-copy-path');
  await c.waitFor(`window.__mock.clipboard.length > 0`);
  assert((await c.evaluate(`window.__mock.clipboard.at(-1)`)) === '/mock/codex-a', 'Copy project path uses the selected session path');

  await c.evaluate(`window.__mock.failAutostartOnce = true; true`);
  await click('#btn-autostart');
  await c.waitFor(`document.getElementById('toast').textContent.includes('Synthetic autostart failure') && !document.getElementById('btn-autostart').disabled`);
  assert(!(await c.evaluate(`autostartOn`)), 'Rejected autostart setup does not switch the UI to enabled');
  assert((await c.evaluate(`document.getElementById('btn-autostart').getAttribute('aria-pressed')`)) === 'false', 'Autostart ARIA state remains disabled after a backend error');
  await click('#btn-autostart');
  await c.waitFor(`autostartOn && !document.getElementById('btn-autostart').disabled`);
  assert(await c.evaluate(`window.__mock.calls.some(call => call.cmd === 'setup_autostart')`), 'Autostart toggle updates enabled state after confirmed system status');
  await click('#btn-autostart');
  await c.waitFor(`!autostartOn && !document.getElementById('btn-autostart').disabled`);
  assert(await c.evaluate(`window.__mock.calls.some(call => call.cmd === 'remove_autostart')`), 'Autostart toggle updates disabled state after confirmed system status');

  await click('#btn-update');
  await c.waitFor(`!document.getElementById('update-modal').hidden && document.getElementById('update-current-version').textContent === '0.1.0'`);
  await c.evaluate(`(() => {
    document.getElementById('update-repository').value = 'example-owner/session-manager';
    document.getElementById('update-auto-check').checked = false;
    window.__mock.updateResult = {
      latest_version: '9.9.9',
      release_name: 'Stable <img src=x onerror=window.__xss=1>',
      release_url: 'https://github.com/example-owner/session-manager/releases/tag/v9.9.9',
      published_at: '2026-11-26T00:00:00Z',
      update_available: true,
    };
    return true;
  })()`);
  await click('#update-save');
  await c.waitFor(`window.__mock.updateSettings.github_repository === 'example-owner/session-manager'`);
  assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'save_update_settings').at(-1).args.autoCheck`) === false, 'Update settings persist the automatic-check switch through IPC');
  await click('#update-check-now');
  await c.waitFor(`!document.getElementById('update-banner').hidden && document.getElementById('update-banner-title').textContent.includes('9.9.9')`);
  assert(await c.evaluate(`document.querySelector('#update-banner img') === null && !window.__xss`), 'Release names render as text rather than remote HTML');
  await c.screenshot('/tmp/csm-ui-update.png');
  await click('#update-close');
  await c.waitFor(`document.getElementById('update-modal').hidden && !document.getElementById('update-banner').hidden`);
  await c.screenshot('/tmp/csm-ui-update-banner.png');
  await click('#btn-update');
  await c.waitFor(`!document.getElementById('update-modal').hidden`);
  await click('#update-open-release');
  await c.waitFor(`window.__mock.openedUpdateUrls.length === 1`);
  assert((await c.evaluate(`window.__mock.openedUpdateUrls[0]`)) === 'https://github.com/example-owner/session-manager/releases/tag/v9.9.9', 'Upgrade action opens the exact validated release URL');
  await click('#update-dismiss');
  await c.waitFor(`document.getElementById('update-banner').hidden && window.__mock.updateSettings.dismissed_version === '9.9.9'`);
  await click('#update-close');
  await c.evaluate(`(async () => { await checkForUpdates(false); return true; })()`);
  assert(await c.evaluate(`document.getElementById('update-banner').hidden`), 'Background checks do not repeatedly show a dismissed version');
  await c.evaluate(`(async () => { await checkForUpdates(true); return true; })()`);
  assert(!(await c.evaluate(`document.getElementById('update-banner').hidden`)), 'A manual check can reopen a dismissed release');
  await click('#update-later');
  await c.evaluate(`(() => {
    window.__mock.updateResult = {
      latest_version: '0.1.0', release_name: 'Current stable',
      release_url: 'https://github.com/example-owner/session-manager/releases/tag/v0.1.0',
      published_at: null, update_available: false,
    };
    return true;
  })()`);
  await c.evaluate(`(async () => { await checkForUpdates(true); return true; })()`);
  assert(await c.evaluate(`document.getElementById('update-banner').hidden`), 'Same-version releases do not show an upgrade banner');
  await c.evaluate(`(() => {
    document.getElementById('toast').className = '';
    document.getElementById('toast').textContent = '';
    window.__mock.updateCheckError = 'Synthetic GitHub offline';
    window.__mock.updateResult.update_available = true;
    window.__mock.updateResult.latest_version = '9.9.9';
    return true;
  })()`);
  await c.evaluate(`(async () => { await checkForUpdates(false); return true; })()`);
  assert(await c.evaluate(`!document.getElementById('toast').classList.contains('show') && !document.getElementById('toast').textContent`), 'Background update failures do not interrupt session work with a toast');
  await c.evaluate(`window.__mock.updateCheckError = ''; true`);
  await click('#btn-update');
  await c.evaluate(`document.getElementById('update-repository').value = ''; true`);
  await click('#update-check-now');
  await c.waitFor(`document.getElementById('update-status').textContent.includes('请先填写')`);
  await click('#update-close');

  await click('[data-filter="all"]');
  await c.waitFor(`filteredList.length === 4`);
  await c.screenshot('/tmp/csm-ui-codex.png');
  assert(await c.evaluate(`document.documentElement.scrollWidth <= 1440`), 'Desktop layout has no horizontal page overflow');
  await c.send('Emulation.setDeviceMetricsOverride', { width: 900, height: 700, deviceScaleFactor: 1, mobile: false });
  await c.screenshot('/tmp/csm-ui-900.png');
  assert(await c.evaluate(`document.documentElement.scrollWidth <= 900`), '900px minimum-width layout has no horizontal page overflow');

  await c.send('Network.setUserAgentOverride', {
    userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0',
  });
  await c.navigate(pageUrl);
  await c.waitFor(`window.__mock && providerStates.claude.loaded && document.querySelectorAll('.session-item').length === 3 && IS_WINDOWS`);
  assert(await c.evaluate(`IS_WINDOWS && MODIFIER_LABEL === 'Ctrl'`), 'Windows UA selects Ctrl modifier labels');
  assert(await c.evaluate(`document.getElementById('btn-selall').title.includes('Ctrl+A')`), 'Windows select-all tooltip uses Ctrl+A');
  assert(await c.evaluate(`[...document.querySelectorAll('span')].every(span => !span.textContent.includes('⌘'))`), 'Windows shortcut hints contain no macOS Command symbols');
  assert(await c.evaluate(`document.querySelector('.empty-features').textContent.includes('Ctrl+F') && document.querySelector('.empty-features').textContent.includes('Ctrl+G')`), 'Windows empty-state shortcuts include Ctrl plus the key');
  assert(await c.evaluate(`document.getElementById('chat-search-btn').title === '会话内搜索 (Ctrl+G)'`), 'Windows chat-search tooltip uses Ctrl+G');
  await c.evaluate(`document.dispatchEvent(new KeyboardEvent('keydown', {key:'f', ctrlKey:true, bubbles:true, cancelable:true}))`);
  assert(await c.evaluate(`document.activeElement?.id === 'search'`), 'Ctrl+F focuses the session search field');

  await c.evaluate(`(() => {
    const state = window.__mock;
    state.blockFavoriteFetch = true;
    state.favoriteFirst = openAllFavorites();
    state.favoriteSecond = openAllFavorites();
    return true;
  })()`);
  await c.evaluate(`new Promise(resolve => setTimeout(resolve, 80))`);
  assert(await callCount('get_favorite_sessions') === 1, 'Double-clicking favorite open starts only one favorites fetch');
  await c.evaluate(`(async () => {
    const state = window.__mock;
    state.blockFavoriteFetch = false;
    state.favoriteFetchResolvers.splice(0).forEach(resolve => resolve());
    await Promise.all([state.favoriteFirst, state.favoriteSecond]);
    delete state.favoriteFirst;
    delete state.favoriteSecond;
    return true;
  })()`);
  assert(await callCount('open_sessions_terminal') === 1, 'Double-clicking favorite open creates only one Terminal batch');
  assert(!(await c.evaluate(`terminalOpening`)), 'Favorite single-flight lock is released after completion');

  await click('[data-filter="fav"]');
  await c.waitFor(`filteredList.length === 1`);
  await c.evaluate(`(async () => {
    await doToggleFav(allSessions.find(s => s.session_id === '${ids.c1}'));
    return true;
  })()`);
  assert(await c.evaluate(`filteredList.length`) === 0, 'Unfavoriting removes the row immediately under the favorites filter');
  assert(await c.evaluate(`document.querySelectorAll('.session-item').length`) === 0, 'Favorites-filter DOM removes the unfavorited row');

  await click('[data-filter="proj"]');
  await c.evaluate(`document.getElementById('search').value = '${ids.c2}'; document.getElementById('search').dispatchEvent(new Event('input', {bubbles:true}))`);
  await c.waitFor(`filteredList.length === 1`);
  assert(await c.evaluate(`document.querySelectorAll('.proj-header').length`) === 1, 'Directory mode finds a session by its Session ID');
  await c.evaluate(`document.getElementById('search').value = ''; document.getElementById('search').dispatchEvent(new Event('input', {bubbles:true}))`);
  await c.waitFor(`document.querySelectorAll('.proj-header').length === 2`);
  await c.evaluate(`(() => {
    allSessions.find(s => s.session_id === '${ids.c1}').title = 'Zulu';
    allSessions.find(s => s.session_id === '${ids.c3}').title = 'Alpha';
    expandedProjects = new Set(['/mock/alpha']);
    sortOrder = 'name';
    applyFilter();
    return true;
  })()`);
  const directoryTitles = await c.evaluate(`[...document.querySelectorAll('.proj-item .s-title')].map(el => el.textContent)`);
  assert(JSON.stringify(directoryTitles) === JSON.stringify(['Alpha', 'Zulu']), 'Directory mode applies name sorting inside each project');

  await c.evaluate(`sortOrder = 'time'; currentFilter = 'all'; applyFilter(); true`);
  await sessionClick(ids.c1);
  await c.waitFor(`activeSession?.session_id === '${ids.c1}' && chatLoaded && chatMsgs.length === 200`);
  await c.evaluate(`(() => {
    chatSearchOpen();
    document.getElementById('chat-search-input').value = 'message-205';
    chatSearchRun();
    return true;
  })()`);
  assert(await c.evaluate(`chatSearchMatches.length === 1 && chatSearchMatches.every(el => el.isConnected)`), 'Chat search initially points at connected highlights');
  await click('#load-more-btn');
  await c.waitFor(`chatMsgs.length === 205 && !chatHasMore`);
  assert(await c.evaluate(`chatSearchMatches.length === 0 && !document.getElementById('chat-search-wrap').classList.contains('visible')`), 'Loading older messages resets chat-search state instead of retaining detached nodes');

  await c.evaluate(`window.__mock.failMessagesOnce.add(${JSON.stringify(sessions[1].file_path)}); true`);
  const c2MessageCallsBefore = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages' && call.args.filePath === ${JSON.stringify(sessions[1].file_path)}).length`);
  await sessionClick(ids.c2);
  await c.waitFor(`activeSession?.session_id === '${ids.c2}' && !chatLoaded && document.querySelector('#chat-area .error')`);
  await sessionClick(ids.c2);
  await c.waitFor(`activeSession?.session_id === '${ids.c2}' && chatLoaded && chatMsgs.length === 2`);
  const c2MessageCallsAfter = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages' && call.args.filePath === ${JSON.stringify(sessions[1].file_path)}).length`);
  assert(c2MessageCallsAfter === c2MessageCallsBefore + 2, 'Clicking the same session retries a failed initial message load');

  assert(c.errors.length === 0, `No browser JavaScript exceptions (${c.errors.length})`);
  const external = c.requests.filter(url => /^https?:\/\//.test(url));
  assert(external.length === 0, `No external HTTP resources are requested (${external.length})`);
  console.log('FRONTEND_SMOKE_OK');
} catch (error) {
  console.error(error.stack || error);
  try { await c.screenshot('/tmp/csm-ui-failure.png'); } catch {}
  process.exitCode = 1;
} finally {
  await c.close();
}
