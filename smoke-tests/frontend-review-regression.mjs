import { connectCdp, assert } from './cdp.mjs';

const ID = '10000000-0000-4000-8000-000000000001';
const OTHER_ID = '10000000-0000-4000-8000-000000000002';
const baseSession = {
  session_id: ID,
  provider: 'claude',
  session_kind: 'main',
  project_slug: 'slug-only',
  project_path: '/mock/project-a',
  title: 'Synthetic A',
  message_count: 2,
  message_count_loaded: true,
  updated_at: '刚才',
  updated_at_ts: 100,
  created_at: '今天',
  created_at_ts: 90,
  file_path: '/mock/a.jsonl',
  favorited: false,
};
const otherSession = {
  ...baseSession,
  session_id: OTHER_ID,
  project_path: '/mock/project-other',
  title: 'Synthetic Other',
  file_path: '/mock/other.jsonl',
  updated_at_ts: 99,
};
const mockSource = `
(() => {
  const state = {
    sessions: [${JSON.stringify(baseSession)}, ${JSON.stringify(otherSession)}],
    messages: {
      '/mock/a.jsonl': [{ role: 'user', text: 'needle A', timestamp: 'now' }],
      '/mock/other.jsonl': [{ role: 'user', text: 'needle Other', timestamp: 'now' }],
      '/mock/b.jsonl': [{ role: 'user', text: 'needle B', timestamp: 'now' }],
    },
    calls: [],
    favorites: new Set(),
    stats: { recoverable: 2, history_ids: 0, history_only: 0, subagent_records: 0 },
    statsFailures: 0,
    deferNextSessions: false,
    sessionResolvers: [],
    deferNextToggle: false,
    toggleResolvers: [],
    deferredMessagePaths: new Set(),
    messageResolvers: new Map(),
    messageFailures: new Map(),
    deferFavoriteReads: 0,
    favoriteReadResolvers: [],
    blockOpens: false,
    openResolvers: [],
    updateSettings: { current_version: '0.1.0', github_repository: '', auto_check: true, dismissed_version: null },
  };
  window.__mock = state;
  const favoriteKey = s => s.provider + ':' + s.session_id;
  const sessionSnapshot = provider => state.sessions.filter(s => s.provider === provider)
    .map(s => ({ ...s, favorited: state.favorites.has(favoriteKey(s)) }));
  window.__TAURI__ = { core: { invoke: async (cmd, args = {}) => {
    state.calls.push({ cmd, args: JSON.parse(JSON.stringify(args)) });
    if (cmd === 'get_sessions') {
      const result = sessionSnapshot(args.provider);
      if (state.deferNextSessions) {
        state.deferNextSessions = false;
        await new Promise(resolve => state.sessionResolvers.push(resolve));
      }
      return result;
    }
    if (cmd === 'get_messages') {
      const failures = state.messageFailures.get(args.filePath) || 0;
      if (failures > 0) {
        state.messageFailures.set(args.filePath, failures - 1);
        throw new Error('synthetic message read failure');
      }
      if (state.deferredMessagePaths.delete(args.filePath)) {
        await new Promise(resolve => state.messageResolvers.set(args.filePath, resolve));
      }
      const all = state.messages[args.filePath] || [];
      const offset = args.offset || 0;
      const snapshotLen = args.snapshotLen ?? all.length;
      const end = Math.max(0, snapshotLen - offset);
      const start = Math.max(0, end - Math.min(args.limit || 200, 1000));
      const messages = all.slice(start, end);
      const next = offset + messages.length;
      return {
        messages, total: start > 0 ? next + 1 : snapshotLen,
        offset: next, next_offset: next, total_exact: start === 0,
        has_more: start > 0, snapshot_len: snapshotLen,
        snapshot_token: args.snapshotToken || ('snapshot:' + args.filePath),
      };
    }
    if (cmd === 'toggle_favorite') {
      if (state.deferNextToggle) {
        state.deferNextToggle = false;
        await new Promise(resolve => state.toggleResolvers.push(resolve));
      }
      const key = args.provider + ':' + args.sessionId;
      if (state.favorites.has(key)) { state.favorites.delete(key); return false; }
      state.favorites.add(key); return true;
    }
    if (cmd === 'open_session') {
      if (state.blockOpens) await new Promise(resolve => state.openResolvers.push(resolve));
      return 'opened';
    }
    if (cmd === 'open_sessions_terminal') return { opened: args.sessions.length, already_running: 0 };
    if (cmd === 'get_claude_storage_stats') {
      if (state.statsFailures > 0) {
        state.statsFailures -= 1;
        throw new Error('synthetic storage permission failure');
      }
      return { ...state.stats };
    }
    if (cmd === 'get_codex_message_count') return 1;
    if (cmd === 'get_favorite_sessions') {
      if (state.deferFavoriteReads > 0) {
        state.deferFavoriteReads -= 1;
        await new Promise(resolve => state.favoriteReadResolvers.push(resolve));
      }
      return sessionSnapshot(args.provider).filter(s => s.favorited);
    }
    if (cmd === 'export_session_markdown') return { file_path: '/mock/export.md', message_count: 1 };
    if (cmd === 'get_autostart_status') return false;
    if (cmd === 'get_update_settings') return { ...state.updateSettings };
    if (cmd === 'save_update_settings') {
      state.updateSettings.github_repository = args.githubRepository || '';
      state.updateSettings.auto_check = args.autoCheck === true;
      state.updateSettings.dismissed_version = null;
      return { ...state.updateSettings };
    }
    if (cmd === 'check_for_updates') throw new Error('Synthetic update check not configured');
    if (cmd === 'dismiss_update') {
      state.updateSettings.dismissed_version = args.version;
      return { ...state.updateSettings };
    }
    if (cmd === 'open_update_release') return null;
    if (cmd === 'should_open_favorites') return false;
    if (cmd === 'reveal_in_finder') return null;
    throw new Error('Unexpected mock command: ' + cmd);
  }}};
})();`;

const c = await connectCdp(Number(process.env.CSM_CDP_PORT || 9223));
const pageUrl = new URL('../src/index.html', import.meta.url).href;
await c.send('Page.addScriptToEvaluateOnNewDocument', { source: mockSource });
const results = [];
const only = process.env.CSM_REVIEW_FILTER || '';

async function fresh() {
  await c.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 960, deviceScaleFactor: 1, mobile: false });
  await c.navigate(pageUrl);
  await c.waitFor(`window.__mock && providerStates.claude.loaded && document.querySelectorAll('.session-item').length === 2`);
}
async function activateA() {
  await c.evaluate(`document.querySelector('.session-item[data-id="${ID}"] .s-body').click()`);
  await c.waitFor(`activeSession?.session_id === '${ID}' && chatLoaded`);
}
async function run(name, body) {
  if (only && only !== name) return;
  try {
    await fresh();
    await body();
    results.push({ name, ok: true });
    console.log(`REVIEW_PASS ${name}`);
  } catch (error) {
    results.push({ name, ok: false, error: String(error?.message || error) });
    console.log(`REVIEW_FAIL ${name}: ${error?.message || error}`);
  }
}

try {
  await run('duplicate-session-identity', async () => {
    await c.evaluate(`(async () => {
      const a = window.__mock.sessions.find(s => s.session_id === '${ID}');
      window.__mock.sessions = [a, { ...a, title: 'Synthetic B', project_path: '/mock/project-b', file_path: '/mock/b.jsonl' }];
      await loadSessions();
    })()`);
    await c.waitFor(`document.querySelectorAll('.session-item').length === 2`);
    await c.evaluate(`document.querySelectorAll('.session-item')[1].querySelector('.s-body').click()`);
    await c.waitFor(`chatLoaded`);
    assert(await c.evaluate(`activeSession?.file_path`) === '/mock/b.jsonl', 'Clicking a duplicate UUID activates the exact backing file');
    assert(await c.evaluate(`chatMsgs[0]?.text`) === 'needle B', 'Duplicate UUID loads the clicked transcript rather than the first match');
    await c.evaluate(`document.querySelectorAll('.session-item')[1].querySelector('.s-check').click()`);
    await c.evaluate(`document.getElementById('btn-terminal').click()`);
    await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'open_sessions_terminal')`);
    const batch = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'open_sessions_terminal').args.sessions`);
    assert(batch.length === 1 && batch[0].file_path === '/mock/b.jsonl', 'Selection and batch opening preserve the exact duplicate-file identity');
  });

  await run('missing-cwd-never-uses-slug', async () => {
    await c.evaluate(`(async () => {
      window.__mock.sessions.find(s => s.session_id === '${ID}').project_path = '';
      await loadSessions();
    })()`);
    await activateA();
    const call = await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages').at(-1)`);
    assert(call.args.projectPath === '', 'Message loading passes an empty real cwd instead of the storage slug');
    await c.evaluate(`showCtxMenu({ clientX: 200, clientY: 200 }, activeSession)`);
    await c.evaluate(`document.getElementById('ctx-export').click()`);
    await c.waitFor(`window.__mock.calls.some(call => call.cmd === 'export_session_markdown')`);
    const exported = await c.evaluate(`window.__mock.calls.find(call => call.cmd === 'export_session_markdown')`);
    assert(exported.args.projectPath === '', 'Export also avoids substituting the storage slug for a missing cwd');
  });

  await run('refresh-retarget-invalidates-chat', async () => {
    await activateA();
    await c.evaluate(`(async () => {
      const a = window.__mock.sessions.find(s => s.session_id === '${ID}');
      window.__mock.sessions = [{ ...a, title: 'Synthetic B', file_path: '/mock/b.jsonl', project_path: '/mock/project-b' }];
      await loadSessions();
    })()`);
    const observed = await c.evaluate(`({ active: activeSession && { id: activeSession.session_id, file: activeSession.file_path }, messages: chatMsgs.length, loaded: chatLoaded })`);
    assert(await c.evaluate(`activeSession === null && chatMsgs.length === 0 && !chatLoaded`), `A backing-file replacement clears the previous transcript and active target; observed=${JSON.stringify(observed)}`);
  });

  await run('markdown-placeholder-collision', async () => {
    const result = await c.evaluate(`(() => {
      try {
        const block = renderMd('\\x00BLOCK0\\x00');
        const span = renderMd('\\x00SPAN99\\x00');
        return { block, span };
      } catch (error) { return { error: String(error) }; }
    })()`);
    assert(!result.error && !result.block.includes('<pre') && !result.span.includes('<code'), 'Literal NUL placeholder text cannot crash or alias Markdown code entries');
  });

  await run('favorite-refresh-races', async () => {
    await c.evaluate(`(() => {
      window.__mock.deferNextSessions = true;
      window.__raceLoad = loadSessions();
    })()`);
    await c.waitFor(`window.__mock.sessionResolvers.length === 1`);
    await c.evaluate(`(async () => { await doToggleFav(allSessions.find(s => s.session_id === '${ID}')); })()`);
    await c.evaluate(`(async () => {
      window.__mock.sessionResolvers.splice(0).forEach(resolve => resolve());
      await window.__raceLoad;
      delete window.__raceLoad;
    })()`);
    assert(await c.evaluate(`allSessions.find(s => s.session_id === '${ID}').favorited`), 'An older list response cannot undo a later confirmed favorite');

    await c.evaluate(`(() => {
      window.__mock.deferNextToggle = true;
      window.__raceFavorite = doToggleFav(allSessions.find(s => s.session_id === '${ID}'));
    })()`);
    await c.waitFor(`window.__mock.toggleResolvers.length === 1`);
    await c.evaluate(`(async () => { await loadSessions(); })()`);
    await c.evaluate(`(async () => {
      window.__mock.toggleResolvers.splice(0).forEach(resolve => resolve());
      await window.__raceFavorite;
      delete window.__raceFavorite;
    })()`);
    assert(await c.evaluate(`!allSessions.find(s => s.session_id === '${ID}').favorited`), 'A toggle result updates the current provider-state object after a refresh');
  });

  await run('single-open-shared-guard', async () => {
    await activateA();
    await c.evaluate(`(() => {
      window.__mock.blockOpens = true;
      window.__firstOpen = doOpen(activeSession);
      window.__secondOpen = doOpen(activeSession);
      window.__batchOpen = doTerminal([activeSession]);
    })()`);
    await c.evaluate(`new Promise(resolve => setTimeout(resolve, 100))`);
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_session').length`) === 1, 'Double-open issues only one single-session request');
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length`) === 0, 'Single and batch actions share the per-session in-flight guard');
    await c.evaluate(`(async () => {
      window.__mock.blockOpens = false;
      window.__mock.openResolvers.splice(0).forEach(resolve => resolve());
      await Promise.all([window.__firstOpen, window.__secondOpen, window.__batchOpen]);
    })()`);
  });

  await run('deleted-active-clears-detail', async () => {
    await activateA();
    await c.evaluate(`(async () => {
      window.__mock.sessions = window.__mock.sessions.filter(s => s.session_id !== '${ID}');
      await loadSessions();
    })()`);
    const observed = await c.evaluate(`({ active: activeSession && { id: activeSession.session_id, file: activeSession.file_path }, messages: chatMsgs.length, loaded: chatLoaded })`);
    assert(await c.evaluate(`activeSession === null && chatMsgs.length === 0 && !chatLoaded`), `Deleting the active transcript clears its detail state; observed=${JSON.stringify(observed)}`);
    assert(await c.evaluate(`getComputedStyle(document.getElementById('open-btn')).display === 'none'`), 'Deleted active sessions no longer expose restore actions');
  });

  await run('markdown-csharp-fsharp', async () => {
    const result = await c.evaluate(`(() => {
      const host = document.createElement('div');
      host.innerHTML = renderMd('\`\`\`c#\\nConsole.WriteLine(1);\\n\`\`\`') + renderMd('\`\`\`f#\\nprintfn "%d" 1\\n\`\`\`');
      return [...host.querySelectorAll('pre')].map(pre => ({ lang: pre.querySelector('.code-lang')?.textContent, text: pre.textContent }));
    })()`);
    assert(result[0].lang === 'c#' && result[1].lang === 'f#', 'C# and F# fence labels retain the hash');
    assert(!result[0].text.startsWith('c##') && !result[0].text.includes('#\\nConsole') && result[0].text.includes('Console.WriteLine(1);'), 'C# code content does not gain a spurious hash line');
    assert(result[1].text.includes('printfn'), 'F# code content remains present');
  });

  await run('list-refresh-preserves-focus', async () => {
    await c.evaluate(`document.querySelectorAll('.session-item')[1].focus()`);
    const before = await c.evaluate(`document.activeElement.dataset.id`);
    await c.evaluate(`(async () => { await loadSessions({ silent: true }); })()`);
    assert(await c.evaluate(`document.activeElement?.classList.contains('session-item') && document.activeElement.dataset.id === ${JSON.stringify(before)} && document.activeElement.isConnected`), 'Silent list refresh restores focus to the same session row');
  });

  await run('chat-arrow-keys-remain-native', async () => {
    await activateA();
    const prevented = await c.evaluate(`(() => {
      const chat = document.getElementById('chat-area');
      chat.focus();
      const event = new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true });
      chat.dispatchEvent(event);
      return event.defaultPrevented || document.activeElement !== chat;
    })()`);
    assert(!prevented, 'ArrowDown in the conversation is not hijacked by session-list navigation');
  });

  await run('context-menu-viewport-clamp', async () => {
    await c.send('Emulation.setDeviceMetricsOverride', { width: 900, height: 700, deviceScaleFactor: 1, mobile: false });
    const rect = await c.evaluate(`(() => {
      const row = document.querySelector('.session-item');
      row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 895, clientY: 695 }));
      const menu = document.getElementById('ctx-menu');
      const rect = menu.getBoundingClientRect();
      return { right: rect.right, bottom: rect.bottom, width: innerWidth, height: innerHeight };
    })()`);
    assert(rect.right <= rect.width && rect.bottom <= rect.height, 'Context menu uses its measured dimensions and stays inside the viewport');
  });

  await run('search-during-initial-load', async () => {
    await c.evaluate(`(() => {
      window.__mock.deferredMessagePaths.add('/mock/a.jsonl');
      window.__pendingActivation = activateSession(allSessions.find(s => s.session_id === '${ID}'));
    })()`);
    await c.waitFor(`document.getElementById('chat-area').getAttribute('aria-busy') === 'true'`);
    await c.evaluate(`chatSearchOpen(); document.getElementById('chat-search-input').value = 'needle'; chatSearchRun()`);
    assert(await c.evaluate(`chatSearchMatches.length`) === 0, 'Synthetic precondition: loading spinner has no searchable messages');
    await c.evaluate(`(async () => {
      window.__mock.messageResolvers.get('/mock/a.jsonl')();
      await window.__pendingActivation;
      delete window.__pendingActivation;
    })()`);
    assert(await c.evaluate(`chatSearchMatches.length > 0 && chatSearchMatches.every(el => el.isConnected)`), 'A query entered during loading is reapplied after messages render');
  });

  await run('chat-search-select-all-shortcut', async () => {
    await activateA();
    const prevented = await c.evaluate(`(() => {
      chatSearchOpen();
      const input = document.getElementById('chat-search-input');
      const event = new KeyboardEvent('keydown', { key: 'a', metaKey: true, bubbles: true, cancelable: true });
      input.dispatchEvent(event);
      return event.defaultPrevented || selectedIds.size !== 0;
    })()`);
    assert(!prevented, 'Cmd/Ctrl+A inside message search is left to the editable field');
  });

  await run('escape-preserves-selection', async () => {
    await activateA();
    await c.evaluate(`selectedIds.add(typeof sessionKey === 'function' ? sessionKey(activeSession) : activeSession.session_id); rerender(); chatSearchOpen()`);
    await c.evaluate(`document.getElementById('chat-search-input').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))`);
    assert(await c.evaluate(`selectedIds.size === 1 && !document.getElementById('chat-search-wrap').classList.contains('visible')`), 'Escape closes message search without clearing the batch selection');
    await c.evaluate(`showCtxMenu({ clientX: 200, clientY: 200 }, activeSession); document.getElementById('ctx-menu').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))`);
    assert(await c.evaluate(`selectedIds.size === 1 && getComputedStyle(document.getElementById('ctx-menu')).display === 'none'`), 'Escape dismisses the context menu without clearing the batch selection');
  });

  await run('silent-refresh-updates-storage-stats', async () => {
    await c.evaluate(`(async () => {
      window.__mock.stats.recoverable = 4;
      await loadSessions({ silent: true });
    })()`);
    await c.waitFor(`document.getElementById('storage-note').textContent.includes('4 可恢复')`);
    assert((await c.evaluate(`document.getElementById('storage-note').textContent`)).includes('4 可恢复'), 'Silent refresh also refreshes Claude storage statistics');
  });

  await run('favorites-open-single-flight', async () => {
    await c.evaluate(`(() => {
      window.__mock.favorites.add('claude:${ID}');
      window.__mock.deferFavoriteReads = 2;
      window.__firstFavoriteOpen = openAllFavorites();
      window.__secondFavoriteOpen = openAllFavorites();
    })()`);
    await c.waitFor(`window.__mock.favoriteReadResolvers.length === 1`);
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_favorite_sessions').length`) === 1, 'The second favorite-open action returns before issuing another favorites read');
    assert(await c.evaluate(`document.getElementById('btn-open-favs').disabled`), 'The favorites button is disabled while its read and Terminal batch are in flight');
    await c.evaluate(`(async () => {
      window.__mock.favoriteReadResolvers.splice(0).forEach(resolve => resolve());
      await Promise.all([window.__firstFavoriteOpen, window.__secondFavoriteOpen]);
      delete window.__firstFavoriteOpen;
      delete window.__secondFavoriteOpen;
    })()`);
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'open_sessions_terminal').length`) === 1, 'Concurrent favorite-open actions produce at most one Terminal batch');
  });

  await run('favorites-filter-removes-unstarred-row', async () => {
    await c.evaluate(`(async () => {
      await doToggleFav(allSessions.find(s => s.session_id === '${ID}'));
      currentFilter = 'fav';
      applyFilter();
    })()`);
    await c.waitFor(`document.querySelectorAll('.session-item').length === 1`);
    await c.evaluate(`(async () => {
      const target = allSessions.find(s => s.session_id === '${ID}');
      selectedIds.add(sessionKey(target));
      await doToggleFav(target);
    })()`);
    assert(await c.evaluate(`filteredList.length === 0 && document.querySelectorAll('.session-item').length === 0`), 'Unstarring immediately removes the row from the active favorites filter');
    assert(await c.evaluate(`selectedIds.size === 0`), 'A row removed by the favorites filter cannot remain invisibly selected');
  });

  await run('pagination-resets-detached-chat-search', async () => {
    await c.evaluate(`window.__mock.messages['/mock/a.jsonl'] = Array.from({ length: 201 }, (_, index) => ({
      role: index % 2 ? 'assistant' : 'user',
      text: (index === 0 || index >= 198 ? 'needle ' : 'filler ') + index,
      timestamp: 'synthetic-' + index,
    }))`);
    await activateA();
    assert(await c.evaluate(`chatMsgs.length === 200 && chatHasMore`), 'Synthetic precondition: the 200-message first page leaves one older message');
    await c.evaluate(`(() => {
      chatSearchOpen();
      document.getElementById('chat-search-input').value = 'needle';
      chatSearchRun();
      window.__oldSearchMatch = chatSearchMatches[0];
    })()`);
    assert(await c.evaluate(`chatSearchMatches.length === 3 && chatSearchMatches.every(el => el.isConnected)`), 'Synthetic precondition: search highlights start connected');
    await c.evaluate(`document.getElementById('load-more-btn').click()`);
    await c.waitFor(`chatMsgs.length === 201 && !chatLoadingMore`);
    const state = await c.evaluate(`(() => {
      const visible = document.getElementById('chat-search-wrap').classList.contains('visible');
      return {
        visible,
        query: document.getElementById('chat-search-input').value,
        matches: chatSearchMatches.length,
        connected: chatSearchMatches.every(el => el.isConnected),
        highlights: document.querySelectorAll('#chat-area .highlight').length,
        oldConnected: window.__oldSearchMatch.isConnected,
      };
    })()`);
    const closedCleanly = !state.visible && state.query === '' && state.matches === 0 && state.highlights === 0;
    const rerunCleanly = state.visible && state.matches > 0 && state.connected && state.highlights === state.matches;
    assert(!state.oldConnected && (closedCleanly || rerunCleanly), `Pagination must close search or rebuild all matches against connected DOM; observed=${JSON.stringify(state)}`);
  });

  await run('directory-search-includes-session-id', async () => {
    await c.evaluate(`(() => {
      currentFilter = 'proj';
      expandedProjects.add('/mock/project-a');
      document.getElementById('search').value = '${ID}';
      applyFilter();
    })()`);
    assert(await c.evaluate(`document.querySelectorAll('.proj-header').length === 1`), 'A Session ID match retains its project directory');
    assert(await c.evaluate(`document.querySelectorAll('.proj-item').length === 1 && document.querySelector('.proj-item').dataset.id === '${ID}'`), 'Directory mode displays the same Session ID match as flat mode');
  });

  await run('directory-name-sort-orders-rows-and-preserves-recency', async () => {
    await c.evaluate(`(async () => {
      const a = window.__mock.sessions.find(s => s.session_id === '${ID}');
      const other = window.__mock.sessions.find(s => s.session_id === '${OTHER_ID}');
      a.title = 'Z newest';
      a.updated_at = 'newest-time';
      other.title = 'A older';
      other.project_path = a.project_path;
      other.updated_at = 'older-time';
      other.updated_at_ts = 99;
      await loadSessions();
      currentFilter = 'proj';
      sortOrder = 'name';
      expandedProjects.add(a.project_path);
      applyFilter();
    })()`);
    const result = await c.evaluate(`({
      titles: [...document.querySelectorAll('.proj-item .s-title')].map(el => el.textContent),
      recent: document.querySelector('.proj-time')?.textContent.trim(),
    })`);
    assert(result.titles.length === 2 && result.titles[0] === 'A older' && result.titles[1] === 'Z newest', `Directory rows honor name sorting; observed=${JSON.stringify(result.titles)}`);
    assert(result.recent === 'newest-time', `Project recency uses the maximum timestamp rather than the first name-sorted row; observed=${result.recent}`);
  });

  await run('chat-search-unicode-offset-mapping', async () => {
    await c.evaluate(`(async () => {
      window.__mock.messages['/mock/a.jsonl'] = [{ role: 'user', text: 'İx 😀x İ', timestamp: 'unicode' }];
      await activateSession(allSessions.find(s => s.session_id === '${ID}'), true);
      chatSearchOpen();
      document.getElementById('chat-search-input').value = 'x';
      chatSearchRun();
    })()`);
    assert(await c.evaluate(`chatSearchMatches.length === 2 && chatSearchMatches.every(el => el.textContent === 'x')`), 'Lowercase expansion cannot shift highlights away from the original UTF-16 offsets');
    assert(await c.evaluate(`document.querySelector('#chat-area .msg-body').textContent === 'İx 😀x İ'`), 'Unicode search preserves the original message text');
    await c.evaluate(`document.getElementById('chat-search-input').value = 'i'; chatSearchRun()`);
    assert(await c.evaluate(`chatSearchMatches.length === 2 && chatSearchMatches.every(el => el.textContent === 'İ')`), 'Matches inside expanded lowercase characters highlight the complete original characters');
  });

  await run('chat-search-unicode-final-sigma', async () => {
    await c.evaluate(`(async () => {
      window.__mock.messages['/mock/a.jsonl'] = [{ role: 'user', text: 'İΣ', timestamp: 'unicode' }];
      await activateSession(allSessions.find(s => s.session_id === '${ID}'), true);
      chatSearchOpen();
      document.getElementById('chat-search-input').value = 'ς';
      chatSearchRun();
    })()`);
    assert(await c.evaluate(`lowercaseWithOriginalOffsets('İΣ').lower === 'İΣ'.toLowerCase()`), 'Offset reconstruction preserves whole-string contextual lowercase semantics');
    assert(await c.evaluate(`chatSearchMatches.length === 1 && chatSearchMatches[0].textContent === 'Σ'`), 'Final-sigma search highlights the original capital sigma after an earlier lowercase expansion');
    await c.evaluate(`document.getElementById('chat-search-input').value = 'Σ'; chatSearchRun()`);
    assert(await c.evaluate(`chatSearchMatches.length === 0`), 'Uppercase sigma lowercases to ordinary σ and must not create a false final-sigma match');
  });

  await run('directory-special-object-keys', async () => {
    await c.evaluate(`(async () => {
      window.__mock.sessions = ['constructor', 'toString', '__proto__'].map((key, index) => ({
        ...${JSON.stringify(baseSession)},
        session_id: '20000000-0000-4000-8000-00000000000' + (index + 1),
        project_path: key,
        file_path: '/mock/' + key + '.jsonl',
        title: key,
      }));
      await loadSessions({ silent: true });
      currentFilter = 'proj';
      applyFilter();
    })()`);
    assert(await c.evaluate(`document.querySelectorAll('.proj-header').length === 3`), 'Project paths that match Object prototype keys still form three independent groups');
    assert(await c.evaluate(`[...document.querySelectorAll('.proj-name')].map(el => el.textContent).sort().join(',') === '__proto__,constructor,toString'`), 'Special project names render without inherited-property type errors');
  });

  await run('storage-error-preserves-and-marks-stale-stats', async () => {
    await c.evaluate(`(async () => {
      window.__mock.statsFailures = 1;
      window.__mock.stats.recoverable = 99;
      await loadSessions({ silent: true });
      await new Promise(resolve => setTimeout(resolve, 0));
    })()`);
    const note = await c.evaluate(`(() => {
      const el = document.getElementById('storage-note');
      return { text: el.textContent, title: el.title, error: el.classList.contains('error') };
    })()`);
    assert(note.text.includes('2 可恢复') && !note.text.includes('99 可恢复'), `A failed stats refresh retains the last good values: ${JSON.stringify(note)}`);
    assert(note.error && note.title.includes('synthetic storage permission failure'), `A failed refresh visibly marks retained stats as stale: ${JSON.stringify(note)}`);
  });

  await run('calendar-groups-across-dst', async () => {
    await c.send('Emulation.setTimezoneOverride', { timezoneId: 'America/New_York' });
    await fresh();
    await c.evaluate(`(() => {
      Date.now = () => new Date(2026, 2, 9, 12, 0, 0).getTime();
      const earlier = { ...${JSON.stringify(baseSession)}, session_id: '${OTHER_ID}', updated_at_ts: new Date(2026, 2, 7, 23, 30, 0).getTime() / 1000 };
      const yesterday = { ...${JSON.stringify(baseSession)}, updated_at_ts: new Date(2026, 2, 8, 12, 0, 0).getTime() / 1000 };
      allSessions = [yesterday, earlier];
      filteredList = [yesterday, earlier];
      sortOrder = 'time';
      renderList();
    })()`);
    const groups = await c.evaluate(`[...document.querySelectorAll('.group-header')].map(el => el.textContent)`);
    assert(JSON.stringify(groups) === JSON.stringify(['昨天', '本周']), `Yesterday and week boundaries use local calendar dates across the 23-hour DST day: ${JSON.stringify(groups)}`);
    await c.send('Emulation.setTimezoneOverride', { timezoneId: '' });
  });

  await run('failed-initial-load-can-retry-same-session', async () => {
    await c.evaluate(`(() => {
      window.__mock.messageFailures.set('/mock/a.jsonl', 1);
      document.querySelector('.session-item[data-id="${ID}"] .s-body').click();
    })()`);
    await c.waitFor(`activeSession?.session_id === '${ID}' && !chatLoaded && document.querySelector('#chat-area [role="alert"]')`);
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages').length`) === 1, 'Synthetic precondition: the first message read failed once');
    await c.evaluate(`document.querySelector('.session-item[data-id="${ID}"] .s-body').click()`);
    await c.waitFor(`chatLoaded && chatMsgs.length === 1`);
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'get_messages').length`) === 2, 'Clicking the same session after a failed initial load issues a fresh read');
    assert(await c.evaluate(`chatMsgs[0]?.text`) === 'needle A', 'The retry renders the recovered transcript');
  });
  await run('update-panel-empty-startup-and-manual-failure', async () => {
    assert(await c.evaluate(`window.__mock.calls.filter(call => call.cmd === 'check_for_updates').length`) === 0, 'An empty repository performs no startup update check');
    await c.evaluate(`document.getElementById('btn-update').click()`);
    await c.waitFor(`!document.getElementById('update-modal').hidden && document.getElementById('update-current-version').textContent === '0.1.0'`);
    await c.evaluate(`document.getElementById('update-repository').value = 'owner/repository'; true`);
    await c.evaluate(`document.getElementById('update-check-now').click()`);
    await c.waitFor(`document.getElementById('update-status').textContent.includes('Synthetic update check not configured')`);
    assert(await c.evaluate(`document.getElementById('update-status').classList.contains('error')`), 'A manual update failure is visible inside its own panel');
    await c.evaluate(`document.getElementById('update-close').click()`);
  });
} finally {
  await c.close();
}

const failed = results.filter(result => !result.ok);
console.log(`REVIEW_REGRESSION_SUMMARY total=${results.length} passed=${results.length - failed.length} failed=${failed.length}`);
for (const result of failed) console.log(`UNFIXED ${result.name}: ${result.error}`);
if (failed.length) process.exitCode = 1;
