import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { SearchAddon } from '@xterm/addon-search';

let invoke, listen, getCurrentWindow;

// HTML escape (XSS 방지)
function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

// Tauri API 초기화
try {
  invoke = window.__TAURI__.core.invoke;
  listen = window.__TAURI__.event.listen;
  getCurrentWindow = window.__TAURI__.window.getCurrentWindow;
  console.log('Tauri API loaded successfully');
} catch (e) {
  console.error('Tauri API not available:', e);
  // 폴백: API 없으면 더미 함수
  invoke = async () => { throw new Error('Tauri not available'); };
  listen = async () => {};
  getCurrentWindow = () => ({ setTitle: () => {}, close: () => {} });
}

// 탭 관리
let tabs = [];
let activeTabId = null;
let tabIdCounter = 0;

// 사이드바 (기본 열림)
let sidebarVisible = true;

// ── 탭 생성 ──

async function createTab(type = 'local', sshConfig = null) {
  const id = tabIdCounter++;
  const container = document.createElement('div');
  container.id = `term-${id}`;
  container.style.width = '100%';
  container.style.height = '100%';
  container.style.display = 'none';
  document.getElementById('terminal-container').appendChild(container);

  const term = new Terminal({
    fontFamily: "'D2Coding', 'SF Mono', 'Menlo', 'Monaco', monospace",
    fontSize: currentFontSize,
    theme: {
      background: currentTheme.bg,
      foreground: currentTheme.fg,
      cursor: currentTheme.cursor,
      selectionBackground: currentTheme.ansi?.selectionBackground || '#264f78',
      black: currentTheme.ansi?.black || '#000000',
      red: currentTheme.ansi?.red || '#cc3333',
      green: currentTheme.ansi?.green || '#33cc33',
      yellow: currentTheme.ansi?.yellow || '#cccc33',
      blue: currentTheme.ansi?.blue || '#4d4de6',
      magenta: currentTheme.ansi?.magenta || '#cc33cc',
      cyan: currentTheme.ansi?.cyan || '#33cccc',
      white: currentTheme.ansi?.white || '#bfbfbf',
      brightBlack: currentTheme.ansi?.brightBlack || '#808080',
      brightRed: currentTheme.ansi?.brightRed || '#ff4d4d',
      brightGreen: currentTheme.ansi?.brightGreen || '#4dff4d',
      brightYellow: currentTheme.ansi?.brightYellow || '#ffff4d',
      brightBlue: currentTheme.ansi?.brightBlue || '#6666ff',
      brightMagenta: currentTheme.ansi?.brightMagenta || '#ff4dff',
      brightCyan: currentTheme.ansi?.brightCyan || '#4dffff',
      brightWhite: currentTheme.ansi?.brightWhite || '#ffffff',
    },
    cursorBlink: true,
    cursorStyle: currentCursorStyle,
    scrollback: 10000,
    allowTransparency: true,
  });

  const fitAddon = new FitAddon();
  const searchAddon = new SearchAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(searchAddon);
  term.loadAddon(new WebLinksAddon());

  term.open(container);

  // WebGL 렌더링 시도
  let webglAddon = null;
  try {
    webglAddon = new WebglAddon();
    term.loadAddon(webglAddon);
  } catch (e) {
    console.warn('WebGL addon failed, using canvas renderer');
  }

  // macOS WKWebView 한글 IME 조합 지원
  // 전략: xterm의 input 이벤트를 container 캡처 단계에서 가로채고,
  //       모든 텍스트 입력을 직접 관리한다. xterm은 특수키만 처리.
  let _imeComposing = false;

  const xtermTextarea = container.querySelector('.xterm-helper-textarea');
  if (xtermTextarea) {
    xtermTextarea.addEventListener('compositionstart', () => {
      _imeComposing = true;
    });

    xtermTextarea.addEventListener('compositionend', (e) => {
      _imeComposing = false;
      if (e.data) {
        invoke('pty_write', { id, data: e.data }).catch(err => {
          console.error('pty_write (IME) failed:', err);
        });
      }
      xtermTextarea.value = '';
    });

    // container 캡처 단계에서 input 이벤트를 가로채 xterm이 보지 못하게 함
    container.addEventListener('input', (e) => {
      if (e.target !== xtermTextarea) return;

      // 조합 중 → xterm 차단 (compositionend에서 처리)
      if (_imeComposing || e.isComposing) {
        e.stopPropagation();
        return;
      }

      // compositionend 직후 insertFromComposition → 이미 처리됨
      if (e.inputType === 'insertFromComposition') {
        e.stopPropagation();
        xtermTextarea.value = '';
        return;
      }

      // 붙여넣기 → xterm onData에서 처리하므로 여기서는 무시 (2중 전송 방지)
      // WKWebView에서는 inputType이 insertFromPaste 대신 insertText로 올 수 있으므로
      // 다중 문자 입력(붙여넣기)은 모두 onData에 위임
      if (e.inputType === 'insertFromPaste' || (e.data && e.data.length > 1)) {
        e.stopPropagation();
        xtermTextarea.value = '';
        return;
      }

      // 일반 텍스트 입력 (영문, 숫자, 기호 등 단일 문자) → 직접 PTY 전송
      if (e.data) {
        e.stopPropagation();
        invoke('pty_write', { id, data: e.data }).catch(err => {
          console.error('pty_write failed:', err);
        });
        xtermTextarea.value = '';
      }
    }, true); // ← capture phase: xterm의 핸들러보다 먼저 실행
  }

  // xterm은 특수키(Enter, 방향키, Ctrl조합 등)만 처리
  // 인쇄 가능한 단일 문자는 위의 input 핸들러가 처리
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== 'keydown') return true;
    if (e.isComposing || e.keyCode === 229 || _imeComposing) return false;
    // Shift+Enter → 개행 (쉘 멀티라인 입력)
    if (e.shiftKey && e.key === 'Enter') {
      e.preventDefault();
      invoke('pty_write', { id, data: '\n' });
      return false;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) return true;
    if (e.key.length > 1) return true; // Enter, Backspace, Arrow, F1~F12 등
    return false; // 인쇄 가능 문자 차단 → input 이벤트로 처리
  });

  fitAddon.fit();

  const tab = {
    id,
    type, // 'local' or 'ssh'
    title: type === 'ssh' ? `${sshConfig?.username}@${sshConfig?.host}` : 'Terminal',
    term,
    fitAddon,
    searchAddon,
    webglAddon,
    container,
    ptyId: null,
    alive: true,
  };

  // Rust 백엔드에 PTY/SSH 생성 요청
  try {
    console.log('Creating PTY:', { id, cols: term.cols, rows: term.rows, type });
    const ptyId = await invoke('create_pty', {
      id,
      cols: term.cols,
      rows: term.rows,
      tabType: type,
      sshConfig: sshConfig || null,
    });
    tab.ptyId = ptyId;
    console.log('PTY created:', ptyId);
  } catch (e) {
    console.error('PTY creation failed:', e);
    term.writeln(`\r\n\x1b[31m[termy] PTY 생성 실패: ${e}\x1b[0m`);
    term.writeln(`\r\n\x1b[33m개발자 도구: ⌘+Option+I\x1b[0m`);
    tab.alive = false;
  }

  // 키 입력 → Rust
  // 텍스트 입력은 위의 input 캡처 핸들러가 처리.
  // onData는 특수키(방향키, Ctrl+C 등)의 이스케이프 시퀀스만 전달.
  term.onData((data) => {
    if (!tab.alive || _imeComposing) return;
    // 인쇄 가능 단일 문자는 input 캡처 핸들러가 이미 전송했으므로 건너뜀
    const code = data.charCodeAt(0);
    if (data.length === 1 && code >= 32 && code !== 127) return;
    invoke('pty_write', { id: tab.id, data }).catch(e => {
      console.error('pty_write failed:', e);
    });
  });

  // 터미널 클릭 시 포커스
  container.addEventListener('click', () => term.focus());

  // 리사이즈 → Rust
  term.onResize(({ cols, rows }) => {
    if (tab.alive) {
      invoke('pty_resize', { id: tab.id, cols, rows });
    }
  });

  tabs.push(tab);
  switchTab(id);
  renderTabBar();

  // 확실한 포커스 + 파일 트리 로드
  setTimeout(() => {
    term.focus();
    loadFilesForTab(id);
    // SSH 접속 시 TMOUT=0 + cd 자동추적(OSC7) 설정 주입
    if (type === 'ssh') {
      injectSshInit(id);
    }
    console.log('Terminal focused, tab:', id);
  }, 500);

  // 윈도우 리사이즈
  const observer = new ResizeObserver(() => {
    if (activeTabId === id) {
      fitAddon.fit();
    }
  });
  observer.observe(container);

  return tab;
}

// ── 탭 전환 ──

function switchTab(id) {
  activeTabId = id;
  tabs.forEach(tab => {
    tab.container.style.display = tab.id === id ? 'block' : 'none';
    if (tab.id === id) {
      setTimeout(() => {
        tab.fitAddon.fit();
        // WebGL 컨텍스트 복구: display:none → block 전환 시 글리프 깨짐 방지
        try {
          tab.term.refresh(0, tab.term.rows - 1);
        } catch (e) {
          // WebGL context lost → addon 재로드
          if (tab.webglAddon) {
            tab.webglAddon.dispose();
            tab.webglAddon = new WebglAddon();
            tab.term.loadAddon(tab.webglAddon);
          }
        }
        tab.term.focus();
        tab.term.scrollToBottom();
      }, 50);
    }
  });
  renderTabBar();
  // 파일 트리도 전환
  if (sidebarVisible) {
    loadFilesForTab(id);
  }
}

// ── 탭 닫기 ──

function closeTab(id) {
  const idx = tabs.findIndex(t => t.id === id);
  if (idx === -1) return;

  invoke('pty_close', { id });
  tabs[idx].term.dispose();
  tabs[idx].container.remove();
  tabs.splice(idx, 1);

  if (tabs.length === 0) {
    getCurrentWindow().close();
    return;
  }

  if (activeTabId === id) {
    switchTab(tabs[Math.min(idx, tabs.length - 1)].id);
  }
  renderTabBar();
}

// ── 탭바 렌더링 ──

function renderTabBar() {
  const tabList = document.getElementById('tab-list');
  tabList.innerHTML = '';

  tabs.forEach(tab => {
    const el = document.createElement('div');
    el.className = `tab ${tab.id === activeTabId ? 'active' : ''}`;

    const icon = tab.type === 'ssh' ? '🌐' : '⌘';
    el.innerHTML = `
      <span class="tab-icon">${icon}</span>
      <span class="tab-title">${escapeHtml(tab.title)}</span>
      <button class="tab-close" title="닫기">×</button>
    `;

    el.querySelector('.tab-title').addEventListener('click', () => switchTab(tab.id));
    el.querySelector('.tab-icon').addEventListener('click', () => switchTab(tab.id));
    el.querySelector('.tab-close').addEventListener('click', (e) => {
      e.stopPropagation();
      closeTab(tab.id);
    });

    // 탭 드래그 순서 변경
    // 우클릭 메뉴
    el.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      showTabMenu(e, tab.id);
    });

    // 마우스 드래그 → 분할 (탭바 밖으로 끌면 분할, 안에서 끌면 순서 변경)
    el.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      if (e.target.closest('.tab-close')) return; // X 버튼은 무시
      initSplitDrag(tab.id, e.clientX, e.clientY, el);
    });

    tabList.appendChild(el);
  });

  // 윈도우 타이틀
  const activeTab = tabs.find(t => t.id === activeTabId);
  if (activeTab) {
    const prefix = tabs.length > 1 ? `[${tabs.indexOf(activeTab) + 1}/${tabs.length}] ` : '';
    getCurrentWindow().setTitle(`${prefix}${activeTab.title} — termy`);
  }

  // 세션 목록 갱신
  renderSessionList();
}

// ── 파일 트리 (사이드바) ──

// 홈 디렉토리 캐시
let _homeDirCache = null;
async function getHomeDir() {
  if (_homeDirCache) return _homeDirCache;
  try { _homeDirCache = await invoke('home_dir'); }
  catch { _homeDirCache = '/tmp'; }
  return _homeDirCache;
}

// 탭 타이틀에서 경로 추출 (예: "user@host:~/path")
function pathFromTitle(title) {
  if (!title) return null;
  const colonIdx = title.lastIndexOf(':');
  if (colonIdx !== -1) {
    let p = title.substring(colonIdx + 1).trim();
    if (p.startsWith('~') && _homeDirCache) p = _homeDirCache + p.slice(1);
    if (p.startsWith('/')) return p;
  }
  const m = title.match(/(\/[\w\-\/.]+)/);
  return m ? m[1] : null;
}

const fileTreeView = {
  els: null,
  state: { tabId: null, type: 'local', cwd: '', files: [], editing: false },
  _suggestItems: [],
  _suggestSelected: -1,
  _suggestTimer: null,
  _composing: false,

  _mount() {
    const sessionList = document.getElementById('session-list');
    if (!sessionList) return null;
    sessionList.innerHTML = '';

    const container = document.createElement('div');
    container.className = 'file-tree-container';

    // 경로 바 (2단: 액션 줄 + 주소창 줄)
    const pathBar = document.createElement('div');
    pathBar.className = 'file-tree-path';

    // 액션 줄
    const actions = document.createElement('div');
    actions.className = 'file-tree-actions';

    const actionsLeft = document.createElement('div');
    actionsLeft.className = 'file-tree-actions-left';

    const upBtn = document.createElement('button');
    upBtn.className = 'file-tree-btn file-tree-up';
    upBtn.title = '상위 폴더';
    upBtn.textContent = '↑';

    const hiddenBtn = document.createElement('button');
    hiddenBtn.className = 'file-tree-btn file-tree-hidden';
    hiddenBtn.title = '숨김 파일 표시';
    hiddenBtn.textContent = '.*';

    actionsLeft.append(upBtn, hiddenBtn);

    const actionsRight = document.createElement('div');
    actionsRight.className = 'file-tree-actions-right';

    const refreshBtn = document.createElement('button');
    refreshBtn.className = 'file-tree-btn file-tree-refresh';
    refreshBtn.title = '새로고침';
    refreshBtn.textContent = '↻';

    const cdBtn = document.createElement('button');
    cdBtn.className = 'file-tree-btn file-tree-cd';
    cdBtn.title = '터미널에서 이 경로로 이동';
    cdBtn.textContent = '▸';

    actionsRight.append(refreshBtn, cdBtn);
    actions.append(actionsLeft, actionsRight);

    // 주소창 줄
    const cwdWrap = document.createElement('div');
    cwdWrap.className = 'file-tree-cwd-wrap';

    const cwdInput = document.createElement('input');
    cwdInput.className = 'file-tree-cwd-input';
    cwdInput.type = 'text';
    cwdInput.spellcheck = false;
    cwdInput.autocomplete = 'off';
    cwdInput.placeholder = '경로 입력 (Tab=자동완성, Enter=이동)';

    const suggest = document.createElement('div');
    suggest.className = 'file-tree-suggest hidden';

    cwdWrap.append(cwdInput, suggest);

    pathBar.append(actions, cwdWrap);

    const list = document.createElement('div');
    list.className = 'file-tree-list';

    const dropZone = document.createElement('div');
    dropZone.className = 'file-tree-dropzone';
    dropZone.textContent = '파일을 여기에 드롭하여 업로드';

    container.append(pathBar, list, dropZone);
    sessionList.appendChild(container);

    this.els = { container, pathBar, upBtn, refreshBtn, hiddenBtn, cwdInput, cwdWrap, suggest, cdBtn, list, dropZone };
    this._bindEvents();
    return this.els;
  },

  _bindEvents() {
    const { upBtn, refreshBtn, hiddenBtn, cwdInput, suggest, cdBtn, list, container } = this.els;

    upBtn.addEventListener('click', () => {
      const cwd = this.state.cwd;
      const parent = cwd.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';
      this.load(this.state.tabId, parent);
    });

    refreshBtn.addEventListener('click', async () => {
      const { tabId, type, cwd } = this.state;
      let realCwd = cwd;
      try {
        if (type === 'ssh') {
          realCwd = (await getPwdFromTerminal(tabId)) || cwd;
        } else {
          realCwd = await invoke('get_pty_cwd', { id: tabId });
        }
      } catch {}
      this.load(tabId, realCwd);
    });

    hiddenBtn.addEventListener('click', () => {
      showHiddenFiles = !showHiddenFiles;
      hiddenBtn.classList.toggle('active', showHiddenFiles);
      this.load(this.state.tabId, this.state.cwd);
    });

    cdBtn.addEventListener('click', () => {
      const tab = tabs.find(t => t.id === this.state.tabId);
      if (!tab || !tab.alive) return;
      const safe = this.state.cwd.replace(/([\\"$`!])/g, '\\$1');
      invoke('pty_write', { id: tab.id, data: `\x15cd "${safe}"\r` });
    });

    // 주소창 입력
    cwdInput.addEventListener('focus', () => {
      this.state.editing = true;
      cwdInput.select();
    });

    cwdInput.addEventListener('blur', () => {
      // 자동완성 클릭 시 blur가 먼저 발생하므로 약간 지연
      setTimeout(() => {
        this.state.editing = false;
        this._hideSuggest();
        cwdInput.value = this.state.cwd;
        cwdInput.classList.remove('error');
      }, 150);
    });

    cwdInput.addEventListener('compositionstart', () => { this._composing = true; });
    cwdInput.addEventListener('compositionend', () => {
      this._composing = false;
      this._scheduleSuggest();
    });

    cwdInput.addEventListener('input', () => {
      if (this._composing) return;
      this._scheduleSuggest();
    });

    cwdInput.addEventListener('keydown', async (e) => {
      if (this._composing) return;

      if (e.key === 'Enter') {
        e.preventDefault();
        // 자동완성 항목이 선택돼 있으면 그걸 우선 적용
        if (this._suggestItems.length > 0 && this._suggestSelected >= 0) {
          const pick = this._suggestItems[this._suggestSelected];
          const target = pick.path;
          if (pick.is_dir) {
            // 폴더면 바로 이동
            const ok = await this._tryNavigate(target);
            if (!ok) this._shake();
          } else {
            // 파일이면 input에 채워넣기만
            cwdInput.value = target;
            this._hideSuggest();
          }
          return;
        }
        const expanded = await this._expandPath(cwdInput.value.trim());
        if (!expanded) return;
        const ok = await this._tryNavigate(expanded);
        if (!ok) this._shake();
      } else if (e.key === 'Escape') {
        e.preventDefault();
        cwdInput.value = this.state.cwd;
        this._hideSuggest();
        cwdInput.blur();
      } else if (e.key === 'Tab') {
        e.preventDefault();
        const items = this._suggestItems;
        if (items.length > 0) {
          const pick = items[Math.max(this._suggestSelected, 0)];
          cwdInput.value = pick.path + (pick.is_dir ? '/' : '');
          this._scheduleSuggest(0);
        } else {
          this._scheduleSuggest(0);
        }
      } else if (e.key === 'ArrowDown') {
        if (this._suggestItems.length === 0) { this._scheduleSuggest(0); return; }
        e.preventDefault();
        this._suggestSelected = Math.min(this._suggestSelected + 1, this._suggestItems.length - 1);
        this._renderSuggestSelection();
      } else if (e.key === 'ArrowUp') {
        if (this._suggestItems.length === 0) return;
        e.preventDefault();
        this._suggestSelected = Math.max(this._suggestSelected - 1, 0);
        this._renderSuggestSelection();
      }
    });

    // 자동완성 패널: blur 방지 + 마우스 hover로 선택 이동 + 클릭 적용
    suggest.addEventListener('mousedown', (e) => e.preventDefault());
    suggest.addEventListener('mousemove', (e) => {
      const item = e.target.closest('.file-tree-suggest-item');
      if (!item) return;
      const idx = Number(item.dataset.idx);
      if (idx !== this._suggestSelected) {
        this._suggestSelected = idx;
        this._renderSuggestSelection();
      }
    });
    suggest.addEventListener('click', (e) => {
      const item = e.target.closest('.file-tree-suggest-item');
      if (!item) return;
      const idx = Number(item.dataset.idx);
      const pick = this._suggestItems[idx];
      if (!pick) return;
      cwdInput.value = pick.path + (pick.is_dir ? '/' : '');
      cwdInput.focus();
      this._scheduleSuggest(0);
    });

    // 파일 목록 이벤트 위임
    list.addEventListener('click', (e) => {
      if (e.target.closest('.file-tree-download')) {
        e.stopPropagation();
        const file = this._fileFromEvent(e);
        if (file) downloadFile(this.state.tabId, file.path, file.name);
      }
    });

    list.addEventListener('dblclick', (e) => {
      const file = this._fileFromEvent(e);
      if (!file) return;
      if (file.is_dir) this.load(this.state.tabId, file.path);
      else downloadFile(this.state.tabId, file.path, file.name);
    });

    list.addEventListener('contextmenu', (e) => {
      const file = this._fileFromEvent(e);
      if (!file) return;
      showContextMenu(e, {
        tabId: this.state.tabId,
        type: this.state.type,
        path: file.path,
        name: file.name,
        isDir: file.is_dir,
        cwd: this.state.cwd,
      });
    });

    // 드래그 앤 드롭 (실제 업로드는 tauri://drag-drop 이벤트에서 처리)
    container.addEventListener('dragover', (e) => {
      e.preventDefault(); e.stopPropagation();
      this.els.dropZone.classList.add('active');
    });
    container.addEventListener('dragleave', (e) => {
      e.preventDefault();
      this.els.dropZone.classList.remove('active');
    });
    container.addEventListener('drop', (e) => {
      e.preventDefault(); e.stopPropagation();
      this.els.dropZone.classList.remove('active');
    });
  },

  _fileFromEvent(e) {
    const el = e.target.closest('.file-tree-item');
    if (!el) return null;
    return this.state.files[Number(el.dataset.idx)] || null;
  },

  async _expandPath(input) {
    if (!input) return '';
    if (input === '~') return await getHomeDir();
    if (input.startsWith('~/')) return (await getHomeDir()) + input.slice(1);
    return input;
  },

  _scheduleSuggest(delay = 120) {
    clearTimeout(this._suggestTimer);
    this._suggestTimer = setTimeout(() => this._runSuggest(), delay);
  },

  async _runSuggest() {
    // 로컬 탭에서만 자동완성
    if (this.state.type !== 'local') { this._hideSuggest(); return; }
    const raw = this.els.cwdInput.value;
    if (!raw) { this._hideSuggest(); return; }
    const expanded = await this._expandPath(raw);

    let parent, prefix;
    if (expanded.endsWith('/')) {
      parent = expanded || '/';
      prefix = '';
    } else {
      const idx = expanded.lastIndexOf('/');
      if (idx < 0) { this._hideSuggest(); return; }
      parent = expanded.slice(0, idx) || '/';
      prefix = expanded.slice(idx + 1).toLowerCase();
    }

    let files;
    try {
      files = await invoke('local_list_dir', { path: parent, showHidden: showHiddenFiles });
    } catch { this._hideSuggest(); return; }

    this._suggestItems = files
      .filter(f => !prefix || f.name.toLowerCase().startsWith(prefix))
      .slice(0, 12);
    this._suggestSelected = this._suggestItems.length > 0 ? 0 : -1;
    this._renderSuggest();
  },

  _renderSuggest() {
    const sg = this.els.suggest;
    if (this._suggestItems.length === 0) { this._hideSuggest(); return; }
    sg.innerHTML = this._suggestItems.map((f, i) => `
      <div class="file-tree-suggest-item${i === this._suggestSelected ? ' selected' : ''}" data-idx="${i}">
        <span class="file-tree-suggest-icon">${f.is_dir ? '📁' : '📄'}</span>
        <span class="file-tree-suggest-name">${escapeHtml(f.name)}</span>
      </div>
    `).join('');
    sg.classList.remove('hidden');
  },

  _renderSuggestSelection() {
    const sg = this.els.suggest;
    sg.querySelectorAll('.file-tree-suggest-item').forEach((el, i) => {
      el.classList.toggle('selected', i === this._suggestSelected);
    });
    const sel = sg.querySelector('.selected');
    if (sel) sel.scrollIntoView({ block: 'nearest' });
  },

  _hideSuggest() {
    this._suggestItems = [];
    this._suggestSelected = -1;
    this.els.suggest.classList.add('hidden');
    this.els.suggest.innerHTML = '';
  },

  async _tryNavigate(expanded) {
    try {
      if (this.state.type === 'ssh') {
        const files = await invoke('sftp_list_dir', { id: this.state.tabId, path: expanded });
        this.update(this.state.tabId, 'ssh', expanded, files);
      } else {
        const files = await invoke('local_list_dir', { path: expanded, showHidden: showHiddenFiles });
        this.update(this.state.tabId, 'local', expanded, files);
      }
      this.state.editing = false;
      this._hideSuggest();
      this.els.cwdInput.blur();
      return true;
    } catch {
      return false;
    }
  },

  _shake() {
    const inp = this.els.cwdInput;
    inp.classList.remove('error');
    void inp.offsetWidth;
    inp.classList.add('error');
  },

  async load(tabId, path) {
    if (!this.els) this._mount();
    if (!this.els) return;
    try {
      const tab = tabs.find(t => t.id === tabId);
      if (!tab) return;
      if (tab.type === 'ssh') {
        if (!path) {
          try { path = await invoke('sftp_get_cwd', { id: tabId }); } catch {}
        }
        const files = await invoke('sftp_list_dir', { id: tabId, path: path || null });
        this.update(tabId, 'ssh', path || '/root', files);
      } else {
        const dir = path || (await getHomeDir());
        const files = await invoke('local_list_dir', { path: dir, showHidden: showHiddenFiles });
        this.update(tabId, 'local', dir, files);
      }
    } catch (e) {
      console.log('파일 트리 로드 실패:', e);
    }
  },

  update(tabId, type, cwd, files) {
    if (!this.els) this._mount();
    if (!this.els) return;

    this.state.tabId = tabId;
    this.state.type = type;
    this.state.cwd = cwd;
    this.state.files = files;

    const { cwdInput, hiddenBtn, list } = this.els;

    // 사용자가 입력 중이면 입력값을 덮어쓰지 않음
    if (!this.state.editing) {
      cwdInput.value = cwd;
      cwdInput.title = cwd;
    }
    hiddenBtn.classList.toggle('active', showHiddenFiles);

    // 파일 목록 (이벤트 위임 → DocumentFragment)
    list.innerHTML = '';
    if (files.length === 0) {
      list.innerHTML = '<div class="sidebar-empty">빈 디렉토리</div>';
      return;
    }
    const frag = document.createDocumentFragment();
    files.forEach((file, idx) => {
      const el = document.createElement('div');
      el.className = 'file-tree-item';
      if (file.is_dir) el.classList.add('is-dir');
      el.dataset.idx = idx;

      const icon = document.createElement('span');
      icon.className = 'file-tree-icon';
      icon.textContent = file.is_dir ? '📁' : '📄';
      el.appendChild(icon);

      const name = document.createElement('span');
      name.className = 'file-tree-name';
      name.textContent = file.name;
      el.appendChild(name);

      if (!file.is_dir) {
        const size = document.createElement('span');
        size.className = 'file-tree-size';
        size.textContent = formatSize(file.size);
        el.appendChild(size);

        const dl = document.createElement('button');
        dl.className = 'file-tree-download';
        dl.title = '다운로드';
        dl.textContent = '↓';
        el.appendChild(dl);
      }
      frag.appendChild(el);
    });
    list.appendChild(frag);
  },
};

// ── 호환 래퍼 (기존 호출 사이트 유지) ──
async function loadRemoteFiles(tabId, path) { await fileTreeView.load(tabId, path); }
async function loadLocalFiles(tabId, path) { await fileTreeView.load(tabId, path); }
async function loadFilesForTab(tabId) { await fileTreeView.load(tabId); }

// Tauri 파일 드롭 이벤트 (Finder에서 드래그)
listen('tauri://drag-drop', async (event) => {
  const paths = event.payload?.paths || [];
  if (paths.length === 0) return;

  const tab = tabs.find(t => t.id === activeTabId);
  if (!tab) return;

  const cwd = fileTreeView.state.cwd || '/';

  for (const localPath of paths) {
    await uploadFile(tab.id, localPath, cwd);
  }
});

async function downloadFile(tabId, remotePath, fileName) {
  try {
    // 저장 위치 선택 다이얼로그
    const { save } = window.__TAURI__.dialog || {};
    let localPath;
    if (save) {
      localPath = await save({
        defaultPath: fileName,
        title: '파일 저장',
      });
    } else {
      // 다이얼로그 없으면 Downloads 폴더
      localPath = `${await getHomeDir()}/Downloads/${fileName}`;
    }
    if (!localPath) return; // 취소

    // 별도 스레드에서 실행되므로 즉시 반환됨 (진행률/완료/에러는 이벤트로 수신)
    await invoke('sftp_download', { id: tabId, remotePath, localPath });
  } catch (e) {
    showToast(`다운로드 실패: ${e}`, true);
  }
}

async function uploadFile(tabId, localPath, remoteDirPath) {
  const fileName = localPath.split('/').pop();
  const remotePath = `${remoteDirPath}/${fileName}`;
  try {
    // 별도 스레드에서 실행되므로 즉시 반환됨 (진행률/완료/에러는 이벤트로 수신)
    await invoke('sftp_upload', { id: tabId, localPath, remotePath });
  } catch (e) {
    showToast(`업로드 실패: ${e}`, true);
  }
}

// 토스트 알림
function showToast(message, isError = false) {
  const toast = document.createElement('div');
  toast.className = `toast ${isError ? 'toast-error' : 'toast-success'}`;
  toast.textContent = message;
  document.body.appendChild(toast);
  setTimeout(() => toast.classList.add('show'), 10);
  setTimeout(() => {
    toast.classList.remove('show');
    setTimeout(() => toast.remove(), 300);
  }, 3000);
}

// 잔상 숨김용 sentinel. 설정 주입 끝에 보이지 않는 OSC로 출력하고,
// 프론트는 이걸 볼 때까지의 출력(에코된 긴 설정 명령)을 숨긴다. xterm은 OSC 1337을 무시.
const TERMY_SENTINEL = '\x1b]1337;termy-ready\x07';

function writeLatin1(term, str) {
  const out = new Uint8Array(str.length);
  for (let i = 0; i < str.length; i++) out[i] = str.charCodeAt(i) & 0xff;
  term.write(out);
}

// SSH 접속/재연결 시 원격 셸에 초기 설정 주입.
// TMOUT=0 + cd 시마다 OSC 7로 현재 경로 자동 출력(bash/zsh). BEL(\007) 종료라 백엔드
// OSC 파서가 그대로 잡는다. 끝에 sentinel을 붙여, 에코된 긴 명령은 화면에서 숨긴다.
function injectSshInit(id) {
  setTimeout(() => {
    const tab = tabs.find(t => t.id === id);
    if (tab) { tab._suppress = ''; tab._suppressing = true; }
    const setup = `\x15export TMOUT=0; if [ -n "$ZSH_VERSION" ]; then autoload -Uz add-zsh-hook 2>/dev/null; __termy7(){ printf '\\033]7;file://%s\\007' "$PWD"; }; add-zsh-hook chpwd __termy7 2>/dev/null; __termy7; elif [ -n "$BASH_VERSION" ]; then __termy7(){ printf '\\033]7;file://%s\\007' "$PWD"; }; case "$PROMPT_COMMAND" in *__termy7*) ;; *) PROMPT_COMMAND="__termy7;$PROMPT_COMMAND";; esac; __termy7; fi; printf '\\033]1337;termy-ready\\007'\r`;
    invoke('pty_write', { id, data: setup });
    // 안전장치: 2.5초 안에 sentinel을 못 보면 버린 출력을 그대로 흘려보내 터미널 손실 방지
    setTimeout(() => {
      const t = tabs.find(x => x.id === id);
      if (t && t._suppressing) {
        t._suppressing = false;
        if (t._suppress) writeLatin1(t.term, t._suppress);
        t._suppress = '';
      }
    }, 2500);
  }, 1000);
}

// su/sudo/중첩 셸 전환 감지 → OSC7 훅 재주입 (su 후에도 트리가 cd를 따라가게).
// su - 는 새 로그인 셸이라 기존 훅이 사라지므로, 새 셸에 훅을 다시 심는다.
// 같은 호스트 파일시스템을 공유하는 셸만 대상 (ssh/docker exec 등 다른 fs는 제외).
const SHELL_SWITCH_RE = /[$#%]\s+(?:su\b|sudo\s+(?:-i|-s|su|bash|zsh|sh)\b|bash\b|zsh\b|dash\b|ksh\b)/;

function trackShellSwitch(tab, arr) {
  if (!tab || tab.type !== 'ssh') return;
  let text = '';
  for (let i = 0; i < arr.length; i++) text += String.fromCharCode(arr[i]);

  // 셸 전환 명령 에코를 보면 재주입 대기 시작
  if (SHELL_SWITCH_RE.test(text)) tab._reinjectPending = Date.now();
  if (!tab._reinjectPending) return;

  // 20초 넘게 안 잠잠해지면 포기
  if (Date.now() - tab._reinjectPending > 20000) {
    tab._reinjectPending = 0;
    clearTimeout(tab._reinjectTimer);
    return;
  }
  // 비밀번호 프롬프트가 보이면 입력이 끝날 때까지 대기 (설정 명령이 비번으로 들어가는 사고 방지)
  if (/[Pp]assword|[Pp]assphrase|암호/.test(text)) {
    clearTimeout(tab._reinjectTimer);
    return;
  }
  // 출력이 잠잠해지면(프롬프트 표시 완료) 훅 재주입
  clearTimeout(tab._reinjectTimer);
  tab._reinjectTimer = setTimeout(() => {
    if (tab._reinjectPending) {
      tab._reinjectPending = 0;
      injectSshInit(tab.id);
    }
  }, 900);
}

// SSH 터미널에서 pwd 실행 후 경로 반환
function getPwdFromTerminal(tabId) {
  return new Promise(async (resolve) => {
    const buffer = [];
    let done = false;

    // pty-output 이벤트 임시 리스너
    const unlisten = await listen('pty-output', (event) => {
      if (event.payload.id !== tabId || done) return;
      const text = new TextDecoder().decode(new Uint8Array(event.payload.data));
      buffer.push(text);
    });

    // 특수 마커를 포함한 pwd 실행 (결과 파싱을 쉽게)
    await invoke('pty_write', { id: tabId, data: '\x15echo "::TERMY_PWD::$(pwd)::TERMY_END::"\r' });

    // 결과 대기
    setTimeout(() => {
      done = true;
      unlisten();

      const output = buffer.join('');
      // 마커 사이의 경로 추출
      const match = output.match(/::TERMY_PWD::(.+?)::TERMY_END::/);
      if (match) {
        resolve(match[1].trim());
      } else {
        // 마커 없으면 / 로 시작하는 줄 찾기
        const lines = output.split(/[\r\n]+/).map(l => l.trim());
        const path = lines.find(l => l.startsWith('/') && !l.includes(' ') && !l.includes('[') && !l.includes('echo'));
        resolve(path || null);
      }
    }, 800);
  });
}

function formatSize(bytes) {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}K`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)}M`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)}G`;
}

function renderSessionList() {
  // 세션 패널 = 활성 탭의 파일 트리 표시 (이미 로드된 경우 재로드 안 함)
  const sessionList = document.getElementById('session-list');
  if (!sessionList) return;
  // 파일 트리가 아직 없으면 로드
  if (!sessionList.querySelector('.file-tree-container') && activeTabId !== null) {
    loadFilesForTab(activeTabId);
  }
}

// ── 사이드바 ──

async function toggleSidebar() {
  sidebarVisible = !sidebarVisible;
  const sidebar = document.getElementById('sidebar');
  sidebar.classList.toggle('hidden', !sidebarVisible);

  if (sidebarVisible) {
    await loadSshList();
  }

  // 모든 탭 리사이즈
  setTimeout(() => {
    tabs.forEach(tab => tab.fitAddon.fit());
  }, 250);
}

async function loadSshList() {
  try {
    const connections = await invoke('get_ssh_connections');
    const list = document.getElementById('ssh-list');
    list.innerHTML = '';

    if (connections.length === 0) {
      list.innerHTML = '<div style="padding: 20px; text-align: center; color: var(--text-dim); font-size: 12px;">저장된 서버 없음<br><br>⌘, 설정에서 추가</div>';
      return;
    }

    sshConnections = connections;
    connections.forEach((conn, i) => {
      // 연결 상태 확인
      const isConnected = tabs.some(t =>
        t.type === 'ssh' && t.alive &&
        t.title.includes(conn.host)
      );
      const el = document.createElement('div');
      el.className = 'ssh-item';
      el.innerHTML = `
        <div class="ssh-item-top">
          <div class="ssh-item-info">
            <div class="ssh-item-name">
              <span class="ssh-item-dot ${isConnected ? 'connected' : ''}"></span>
              ${escapeHtml(conn.name || conn.host)}
            </div>
            <div class="ssh-item-host">${escapeHtml(conn.username)}@${escapeHtml(conn.host)}:${conn.port}</div>
          </div>
          <div class="ssh-item-actions">
            <button class="ssh-item-btn ssh-item-connect" title="연결">연결</button>
            <button class="ssh-item-btn ssh-item-edit" title="편집">편집</button>
          </div>
        </div>
      `;
      el.querySelector('.ssh-item-connect').addEventListener('click', (e) => {
        e.stopPropagation();
        createTab('ssh', conn);
        document.querySelector('.sidebar-tab[data-tab="sessions"]')?.click();
      });
      el.querySelector('.ssh-item-edit').addEventListener('click', (e) => {
        e.stopPropagation();
        openSshModal(conn, i);
      });
      // 더블클릭도 접속
      el.addEventListener('dblclick', () => {
        createTab('ssh', conn);
        document.querySelector('.sidebar-tab[data-tab="sessions"]')?.click();
      });
      list.appendChild(el);
    });
  } catch (e) {
    console.error('Failed to load SSH list:', e);
  }
}

// ── Rust에서 데이터 수신 ──

listen('pty-output', (event) => {
  const { id, data } = event.payload;
  const tab = tabs.find(t => t.id === id);
  if (!tab) return;
  const arr = new Uint8Array(data);
  // 접속 직후 설정 명령 잔상 숨김: sentinel을 볼 때까지 버퍼링 후 그 뒤만 출력
  if (tab._suppressing) {
    let s = '';
    for (let i = 0; i < arr.length; i++) s += String.fromCharCode(arr[i]);
    tab._suppress += s;
    const idx = tab._suppress.indexOf(TERMY_SENTINEL);
    if (idx !== -1) {
      const rest = tab._suppress.slice(idx + TERMY_SENTINEL.length);
      tab._suppressing = false;
      tab._suppress = '';
      // 숨긴 명령 대신 줄바꿈 하나만 넣어 다음 프롬프트를 깨끗한 줄에 표시
      writeLatin1(tab.term, '\r\n' + rest);
    }
    return;
  }
  tab.term.write(arr);
  trackShellSwitch(tab, arr);
}).then(() => console.log('Listening for pty-output'));

listen('pty-error', (event) => {
  const { id, error } = event.payload;
  console.error('PTY error:', id, error);
  let tab = tabs.find(t => t.id === id);
  if (tab) {
    tab.term.writeln(`\r\n\x1b[31m[termy] ${error}\x1b[0m`);
  }
}).then(() => console.log('Listening for pty-error'));

listen('pty-exit', (event) => {
  const { id } = event.payload;
  console.log('PTY exit:', id);
  let tab = tabs.find(t => t.id === id);
  if (tab) {
    tab.alive = false;
    tab.title += ' (종료됨)';
    renderTabBar();
  }
}).then(() => console.log('Listening for pty-exit'));

listen('pty-title', (event) => {
  const { id, title } = event.payload;
  const tab = tabs.find(t => t.id === id);
  if (tab) {
    tab.title = title;
    renderTabBar();

    // 활성 탭이면 파일 트리도 갱신 (로컬 탭만, SSH는 pty-cwd에서 처리)
    if (id === activeTabId && tab.type !== 'ssh') {
      const path = pathFromTitle(title);
      if (path) {
        fileTreeView.load(id, path);
      }
    }
  }
}).then(() => console.log('Listening for pty-title'));

// SSH 터미널 cd → SFTP 파일트리 자동 추적 (OSC 7 기반)
listen('pty-cwd', (event) => {
  const { id, cwd } = event.payload;
  const tab = tabs.find(t => t.id === id);
  // 활성 SSH 탭이고, 트리가 보고 있는 경로와 다를 때만 갱신 (엔터 연타 시 중복 요청 방지)
  if (tab && tab.type === 'ssh' && id === activeTabId && fileTreeView.state.cwd !== cwd) {
    fileTreeView.load(id, cwd);
  }
}).then(() => console.log('Listening for pty-cwd'));

// SSH 자동 재연결 성공 → cd 추적 설정 재주입 + 파일트리 재로드
listen('pty-reconnected', (event) => {
  const { id } = event.payload;
  const tab = tabs.find(t => t.id === id);
  if (tab && tab.type === 'ssh') {
    injectSshInit(id);
    if (id === activeTabId) {
      setTimeout(() => fileTreeView.load(id), 1500);
    }
  }
}).then(() => console.log('Listening for pty-reconnected'));

// ── 키보드 단축키 ──

document.addEventListener('keydown', (e) => {
  if (e.metaKey) {
    switch (e.key) {
      case 't':
        e.preventDefault();
        createTab();
        break;
      case 'w':
        e.preventDefault();
        if (activeTabId !== null) closeTab(activeTabId);
        break;
      case 'b':
        e.preventDefault();
        toggleSidebar();
        break;
      case ',':
        e.preventDefault();
        openSettingsModal();
        break;
      case 'd':
        e.preventDefault();
        if (e.shiftKey) {
          toggleSplit('vertical');
        } else {
          toggleSplit('horizontal');
        }
        break;
      case 'ArrowLeft':
      case 'ArrowUp':
        if (e.altKey && splitState) {
          e.preventDefault();
          focusSplitPane('left');
        }
        break;
      case 'ArrowRight':
      case 'ArrowDown':
        if (e.altKey && splitState) {
          e.preventDefault();
          focusSplitPane('right');
        }
        break;
      case 'f':
        e.preventDefault();
        toggleSearch();
        break;
      case '[':
        e.preventDefault();
        navigateTab(-1);
        break;
      case ']':
        e.preventDefault();
        navigateTab(1);
        break;
    }
    // ⌘+/⌘- 폰트 크기
    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      currentFontSize = Math.min(32, currentFontSize + 1);
      tabs.forEach(t => { t.term.options.fontSize = currentFontSize; t.fitAddon.fit(); });
      showToast(`폰트 ${currentFontSize}px`);
    }
    if (e.key === '-') {
      e.preventDefault();
      currentFontSize = Math.max(8, currentFontSize - 1);
      tabs.forEach(t => { t.term.options.fontSize = currentFontSize; t.fitAddon.fit(); });
      showToast(`폰트 ${currentFontSize}px`);
    }
    if (e.key === '0') {
      e.preventDefault();
      currentFontSize = 14;
      tabs.forEach(t => { t.term.options.fontSize = currentFontSize; t.fitAddon.fit(); });
      showToast(`폰트 초기화 ${currentFontSize}px`);
    }
    // ⌘+1~9
    if (e.key >= '1' && e.key <= '9') {
      e.preventDefault();
      const idx = parseInt(e.key) - 1;
      if (idx < tabs.length) switchTab(tabs[idx].id);
    }
  }
});

function navigateTab(direction) {
  const idx = tabs.findIndex(t => t.id === activeTabId);
  const newIdx = (idx + direction + tabs.length) % tabs.length;
  switchTab(tabs[newIdx].id);
}

// ── 버튼 이벤트 ──

document.getElementById('new-tab-btn').addEventListener('click', () => createTab());
document.getElementById('sidebar-add-btn').addEventListener('click', () => openSshModal());
document.getElementById('settings-btn').addEventListener('click', () => openSettingsModal());

document.getElementById('sidebar-toggle-btn').addEventListener('click', () => {
  toggleSidebar();
  document.getElementById('sidebar-toggle-btn').classList.toggle('active', sidebarVisible);
  renderSessionList();
});

// ── 전송 진행률 ──

listen('transfer-progress', (event) => {
  const { type, name, progress, downloaded, uploaded, total } = event.payload;
  const bar = document.getElementById('transfer-bar');
  bar.classList.remove('hidden');

  let item = bar.querySelector(`[data-name="${CSS.escape(name)}"]`);
  if (!item) {
    item = document.createElement('div');
    item.className = 'transfer-item';
    item.dataset.name = name;
    item.innerHTML = `
      <span class="transfer-icon">${type === 'download' ? '↓' : '↑'}</span>
      <div class="transfer-info">
        <div class="transfer-name">${name}</div>
        <div class="transfer-detail"></div>
        <div class="transfer-progress"><div class="transfer-progress-fill"></div></div>
      </div>
    `;
    bar.appendChild(item);
  }

  const transferred = type === 'download' ? downloaded : uploaded;
  item.querySelector('.transfer-detail').textContent = `${formatSize(transferred)} / ${formatSize(total)} (${progress}%)`;
  item.querySelector('.transfer-progress-fill').style.width = `${progress}%`;
});

listen('transfer-complete', (event) => {
  const { type, name, tabId } = event.payload;
  const label = type === 'download' ? '다운로드' : '업로드';
  showToast(`${label} 완료: ${name}`);

  const bar = document.getElementById('transfer-bar');
  const item = bar.querySelector(`[data-name="${CSS.escape(name)}"]`);
  if (item) {
    item.querySelector('.transfer-detail').textContent = `완료`;
    item.querySelector('.transfer-progress-fill').style.width = '100%';
    item.querySelector('.transfer-progress-fill').classList.add('complete');
    // 3초 후 제거
    setTimeout(() => {
      item.remove();
      if (bar.children.length === 0) bar.classList.add('hidden');
    }, 3000);
  }

  // 업로드 완료 시 해당 탭의 파일 트리 새로고침
  if (type === 'upload' && tabId != null) {
    const uploadTab = tabs.find(t => t.id === tabId);
    if (uploadTab && uploadTab.sftpCwd) {
      if (uploadTab.type === 'ssh') loadRemoteFiles(tabId, uploadTab.sftpCwd);
      else loadLocalFiles(tabId, uploadTab.sftpCwd);
    }
  }
});

listen('transfer-error', (event) => {
  const { type, name, error } = event.payload;
  const label = type === 'download' ? '다운로드' : '업로드';
  showToast(`${label} 실패: ${error}`, true);

  const bar = document.getElementById('transfer-bar');
  const item = bar.querySelector(`[data-name="${CSS.escape(name)}"]`);
  if (item) {
    item.querySelector('.transfer-detail').textContent = `실패`;
    item.querySelector('.transfer-progress-fill').classList.add('error');
    setTimeout(() => {
      item.remove();
      if (bar.children.length === 0) bar.classList.add('hidden');
    }, 5000);
  }
});

// 원격 편집 자동 저장 동기화 알림
listen('edit-synced', (event) => {
  showToast(`서버에 저장됨: ${event.payload.name}`);
});
listen('edit-error', (event) => {
  showToast(`저장 실패: ${event.payload.name} (${event.payload.error})`, true);
});

// ── 사이드바 탭 전환 ──

document.querySelectorAll('.sidebar-tab').forEach(tab => {
  tab.addEventListener('click', () => {
    // 탭 활성화
    document.querySelectorAll('.sidebar-tab').forEach(t => t.classList.remove('active'));
    tab.classList.add('active');

    // 패널 전환
    document.querySelectorAll('.sidebar-panel').forEach(p => p.classList.remove('active'));
    const panelId = `panel-${tab.dataset.tab}`;
    document.getElementById(panelId)?.classList.add('active');

    // 서버 탭 선택 시 목록 새로고침
    if (tab.dataset.tab === 'servers') {
      loadSshList();
    }
  });
});

// ── 테마 데이터 ──

const THEMES = [
  { name: 'termy Navy', bg: '#1a1a2e', fg: '#d4d8e8', cursor: '#0078d4',
    ansi: {
      black: '#0f0f1a', red: '#e05561', green: '#43d08a', yellow: '#e6c07b',
      blue: '#0078d4', magenta: '#c678dd', cyan: '#56b6c2', white: '#d4d8e8',
      brightBlack: '#3b3d5e', brightRed: '#ff6b7a', brightGreen: '#5af5a0',
      brightYellow: '#ffd68a', brightBlue: '#3a9eea', brightMagenta: '#dc8ef5',
      brightCyan: '#6fd4df', brightWhite: '#f0f2f8',
      selectionBackground: '#0078d450',
    }
  },
  { name: 'termy Dark', bg: '#0d1117', fg: '#d9d9d9', cursor: '#58a6ff' },
  { name: 'Dracula', bg: '#282a36', fg: '#f8f8f2', cursor: '#bd93f9' },
  { name: 'Nord', bg: '#2e3440', fg: '#d8dee9', cursor: '#88c0d0' },
  { name: 'Monokai', bg: '#272822', fg: '#f8f8f2', cursor: '#f92672' },
  { name: 'One Dark', bg: '#282c34', fg: '#abb2bf', cursor: '#528bff' },
  { name: 'Tokyo Night', bg: '#1a1b26', fg: '#c0caf5', cursor: '#7aa2f7' },
  { name: 'Gruvbox', bg: '#282828', fg: '#ebdbb2', cursor: '#fe8019' },
  { name: 'Catppuccin', bg: '#1e1e2e', fg: '#cdd6f4', cursor: '#b4befe' },
  { name: 'Solarized', bg: '#002b36', fg: '#839496', cursor: '#268bd2' },
  { name: 'GitHub Dark', bg: '#0d1117', fg: '#c9d1d9', cursor: '#58a6ff' },
  { name: 'Rosé Pine', bg: '#191724', fg: '#e0def4', cursor: '#c4a7e7' },
  { name: 'Light', bg: '#ffffff', fg: '#24292f', cursor: '#0969da' },
];

let currentTheme = THEMES[0];
let currentFontSize = 14;
let currentCursorStyle = 'bar';
let showHiddenFiles = false;

// ── 설정 모달 ──

function openSettingsModal() {
  const modal = document.getElementById('settings-modal');
  modal.classList.remove('hidden');

  // 테마 그리드 생성
  const grid = document.getElementById('theme-grid');
  grid.innerHTML = '';
  THEMES.forEach((theme, i) => {
    const card = document.createElement('div');
    card.className = `theme-card ${theme.name === currentTheme.name ? 'active' : ''}`;
    card.innerHTML = `
      <div class="theme-card-preview" style="background:${theme.bg};color:${theme.fg}">
        ~ $ ls<br>app src
      </div>
      <div class="theme-card-name">${theme.name}</div>
    `;
    card.addEventListener('click', () => {
      grid.querySelectorAll('.theme-card').forEach(c => c.classList.remove('active'));
      card.classList.add('active');
      currentTheme = theme;
      applyThemePreview(theme);
    });
    grid.appendChild(card);
  });

  // 폰트 크기
  const slider = document.getElementById('settings-fontsize');
  slider.value = currentFontSize;
  document.getElementById('settings-fontsize-label').textContent = `${currentFontSize}px`;

  // 커서 스타일
  document.querySelectorAll('.cursor-tab').forEach(t => {
    t.classList.toggle('active', t.dataset.cursor === currentCursorStyle);
  });
}

function closeSettingsModal() {
  document.getElementById('settings-modal').classList.add('hidden');
}

async function saveSettings() {
  currentFontSize = parseInt(document.getElementById('settings-fontsize').value);
  const scrollback = parseInt(document.getElementById('settings-scrollback').value) || 10000;

  // 모든 탭에 적용
  tabs.forEach(tab => {
    tab.term.options.fontSize = currentFontSize;
    tab.term.options.cursorStyle = currentCursorStyle;
    tab.term.options.scrollback = scrollback;
    tab.term.options.theme = {
      background: currentTheme.bg,
      foreground: currentTheme.fg,
      cursor: currentTheme.cursor,
      selectionBackground: currentTheme.ansi?.selectionBackground || '#264f78',
      ...(currentTheme.ansi ? {
        black: currentTheme.ansi.black, red: currentTheme.ansi.red,
        green: currentTheme.ansi.green, yellow: currentTheme.ansi.yellow,
        blue: currentTheme.ansi.blue, magenta: currentTheme.ansi.magenta,
        cyan: currentTheme.ansi.cyan, white: currentTheme.ansi.white,
        brightBlack: currentTheme.ansi.brightBlack, brightRed: currentTheme.ansi.brightRed,
        brightGreen: currentTheme.ansi.brightGreen, brightYellow: currentTheme.ansi.brightYellow,
        brightBlue: currentTheme.ansi.brightBlue, brightMagenta: currentTheme.ansi.brightMagenta,
        brightCyan: currentTheme.ansi.brightCyan, brightWhite: currentTheme.ansi.brightWhite,
      } : {}),
    };
    tab.fitAddon.fit();
  });

  // UI 전체 테마 적용
  applyUiTheme(currentTheme);

  // 설정 파일에 영속 저장
  try {
    await invoke('save_app_settings', {
      settings: {
        theme: currentTheme.name,
        fontSize: currentFontSize,
        cursorStyle: currentCursorStyle,
        scrollback,
        showHiddenFiles: showHiddenFiles,
      }
    });
  } catch (e) {
    console.error('Settings save failed:', e);
  }

  showToast('설정 저장됨');
  closeSettingsModal();
}

function applyThemePreview(theme) {
  // 터미널에 즉시 프리뷰
  const tab = tabs.find(t => t.id === activeTabId);
  if (tab) {
    tab.term.options.theme = {
      ...tab.term.options.theme,
      background: theme.bg,
      foreground: theme.fg,
      cursor: theme.cursor,
    };
  }
  // 사이드바 + 탭바 + 전체 UI 테마 적용
  applyUiTheme(theme);
}

function applyUiTheme(theme) {
  const root = document.documentElement;
  // 테마 배경을 기준으로 밝기 계산
  const r = parseInt(theme.bg.slice(1,3), 16);
  const g = parseInt(theme.bg.slice(3,5), 16);
  const b = parseInt(theme.bg.slice(5,7), 16);

  // 사이드바/탭바 색상을 테마 기반으로 조정
  const darken = (hex, amount) => {
    const rr = Math.max(0, parseInt(hex.slice(1,3), 16) - amount);
    const gg = Math.max(0, parseInt(hex.slice(3,5), 16) - amount);
    const bb = Math.max(0, parseInt(hex.slice(5,7), 16) - amount);
    return `rgb(${rr},${gg},${bb})`;
  };
  const lighten = (hex, amount) => {
    const rr = Math.min(255, parseInt(hex.slice(1,3), 16) + amount);
    const gg = Math.min(255, parseInt(hex.slice(3,5), 16) + amount);
    const bb = Math.min(255, parseInt(hex.slice(5,7), 16) + amount);
    return `rgb(${rr},${gg},${bb})`;
  };

  root.style.setProperty('--bg-primary', theme.bg);
  root.style.setProperty('--bg-secondary', darken(theme.bg, 5));
  root.style.setProperty('--bg-tertiary', lighten(theme.bg, 10));
  root.style.setProperty('--bg-sidebar', darken(theme.bg, 10));
  root.style.setProperty('--bg-hover', lighten(theme.bg, 15));
  root.style.setProperty('--bg-active', lighten(theme.bg, 20));
  root.style.setProperty('--border', lighten(theme.bg, 25));
  root.style.setProperty('--text-primary', theme.fg);
  root.style.setProperty('--text-secondary', lighten(theme.bg, 100));
  root.style.setProperty('--text-dim', lighten(theme.bg, 60));
  root.style.setProperty('--accent', theme.cursor);
  root.style.setProperty('--tab-active-border', theme.cursor);
}

// 설정 모달 이벤트
document.getElementById('settings-modal-close').addEventListener('click', closeSettingsModal);
document.getElementById('settings-modal-cancel').addEventListener('click', closeSettingsModal);
document.getElementById('settings-modal-save').addEventListener('click', saveSettings);
document.getElementById('settings-modal').addEventListener('click', (e) => {
  if (e.target === e.currentTarget) closeSettingsModal();
});

document.getElementById('settings-fontsize').addEventListener('input', (e) => {
  const size = e.target.value;
  document.getElementById('settings-fontsize-label').textContent = `${size}px`;
  // 실시간 프리뷰
  const tab = tabs.find(t => t.id === activeTabId);
  if (tab) {
    tab.term.options.fontSize = parseInt(size);
    tab.fitAddon.fit();
  }
});

document.querySelectorAll('.cursor-tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.cursor-tab').forEach(t => t.classList.remove('active'));
    tab.classList.add('active');
    currentCursorStyle = tab.dataset.cursor;
    // 실시간 프리뷰
    const activeTab = tabs.find(t => t.id === activeTabId);
    if (activeTab) activeTab.term.options.cursorStyle = currentCursorStyle;
  });
});

// ── SSH 모달 ──

let sshConnections = [];
let editingConnectionIndex = -1; // -1 = 새로 추가

function openSshModal(connection = null, index = -1) {
  editingConnectionIndex = index;
  const modal = document.getElementById('ssh-modal');
  modal.classList.remove('hidden');

  document.getElementById('ssh-modal-title').textContent = connection ? '연결 편집' : '새 SSH 연결';
  document.getElementById('ssh-name').value = connection?.name || '';
  document.getElementById('ssh-host').value = connection?.host || '';
  document.getElementById('ssh-port').value = connection?.port || 22;
  document.getElementById('ssh-user').value = connection?.username || '';
  document.getElementById('ssh-keypath').value = connection?.keyPath || '';

  // 인증 탭
  const authType = connection?.authType || 'agent';
  document.querySelectorAll('.modal-auth-tab').forEach(t => {
    t.classList.toggle('active', t.dataset.auth === authType);
  });
  document.getElementById('ssh-key-field').classList.toggle('hidden', authType !== 'key');
  document.getElementById('ssh-password-field').classList.toggle('hidden', authType !== 'password');
  document.getElementById('ssh-password').value = '';

  // 삭제 버튼
  document.getElementById('ssh-modal-delete').classList.toggle('hidden', index === -1);

  // 호스트 필드에 포커스
  setTimeout(() => document.getElementById('ssh-host').focus(), 100);
}

function closeSshModal() {
  document.getElementById('ssh-modal').classList.add('hidden');
}

async function saveSshModal() {
  const name = document.getElementById('ssh-name').value.trim();
  const host = document.getElementById('ssh-host').value.trim();
  const port = parseInt(document.getElementById('ssh-port').value) || 22;
  const username = document.getElementById('ssh-user').value.trim();
  const keyPath = document.getElementById('ssh-keypath').value.trim();
  const authType = document.querySelector('.modal-auth-tab.active')?.dataset.auth || 'agent';

  if (!host || !username) {
    showToast('호스트와 사용자를 입력하세요', true);
    return;
  }

  const password = document.getElementById('ssh-password').value;
  const conn = { name, host, port, username, authType, keyPath };

  if (editingConnectionIndex >= 0) {
    sshConnections[editingConnectionIndex] = conn;
  } else {
    sshConnections.push(conn);
  }

  try {
    await invoke('save_ssh_connections', { connections: sshConnections });
    // 비밀번호가 있으면 키체인에 저장
    if (password && authType === 'password') {
      await invoke('save_password_to_keychain', { host, username, password });
    }
    showToast('저장 완료');
  } catch (e) {
    showToast(`저장 실패: ${e}`, true);
  }

  closeSshModal();
  loadSshList();
}

async function deleteSshConnection() {
  if (editingConnectionIndex >= 0) {
    sshConnections.splice(editingConnectionIndex, 1);
    try {
      await invoke('save_ssh_connections', { connections: sshConnections });
      showToast('삭제 완료');
    } catch (e) {
      showToast(`삭제 실패: ${e}`, true);
    }
    closeSshModal();
    loadSshList();
  }
}

// 모달 이벤트
document.getElementById('ssh-modal-close').addEventListener('click', closeSshModal);
document.getElementById('ssh-modal-cancel').addEventListener('click', closeSshModal);
document.getElementById('ssh-modal-save').addEventListener('click', saveSshModal);
document.getElementById('ssh-modal-delete').addEventListener('click', deleteSshConnection);
document.getElementById('ssh-modal').addEventListener('click', (e) => {
  if (e.target === e.currentTarget) closeSshModal(); // 오버레이 클릭 시 닫기
});

// 인증 방식 탭 전환
document.querySelectorAll('.modal-auth-tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.modal-auth-tab').forEach(t => t.classList.remove('active'));
    tab.classList.add('active');
    document.getElementById('ssh-key-field').classList.toggle('hidden', tab.dataset.auth !== 'key');
    document.getElementById('ssh-password-field').classList.toggle('hidden', tab.dataset.auth !== 'password');
  });
});

// Enter 키로 저장
document.querySelectorAll('#ssh-modal input').forEach(input => {
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') saveSshModal();
    if (e.key === 'Escape') closeSshModal();
  });
});

// ── 분할 화면 (v2) ──

let splitState = null; // { rightTabId, resizeEl }

async function toggleSplit(direction = 'horizontal') {
  const container = document.getElementById('terminal-container');

  // 이미 분할 → 해제
  if (splitState) {
    unsplit();
    return;
  }

  const isHorizontal = direction === 'horizontal';
  const firstTab = tabs.find(t => t.id === activeTabId);
  if (!firstTab) return;

  // 새 탭 생성
  await createTab();
  const secondTab = tabs[tabs.length - 1];

  // 컨테이너를 flex로
  container.style.display = 'flex';
  container.style.flexDirection = isHorizontal ? 'row' : 'column';

  // 첫 번째 탭
  firstTab.container.style.display = 'block';
  firstTab.container.style[isHorizontal ? 'width' : 'height'] = '50%';
  firstTab.container.style[isHorizontal ? 'height' : 'width'] = '100%';
  firstTab.container.style.flex = 'none';

  // 리사이즈 바
  const resizeBar = document.createElement('div');
  resizeBar.id = 'split-resize-bar';
  if (isHorizontal) {
    resizeBar.style.cssText = 'width:4px;height:100%;cursor:col-resize;background:#333;flex-shrink:0;transition:background 0.15s;';
  } else {
    resizeBar.style.cssText = 'height:4px;width:100%;cursor:row-resize;background:#333;flex-shrink:0;transition:background 0.15s;';
  }
  let isResizingSplit = false;
  resizeBar.addEventListener('mouseenter', () => resizeBar.style.background = '#0078d4');
  resizeBar.addEventListener('mouseleave', () => { if (!isResizingSplit) resizeBar.style.background = '#333'; });

  // 두 번째 탭
  secondTab.container.style.display = 'block';
  secondTab.container.style[isHorizontal ? 'width' : 'height'] = '50%';
  secondTab.container.style[isHorizontal ? 'height' : 'width'] = '100%';
  secondTab.container.style.flex = 'none';

  // DOM에 추가
  container.appendChild(resizeBar);
  container.appendChild(secondTab.container);

  // 리사이즈 핸들러
  resizeBar.addEventListener('mousedown', (e) => {
    isResizingSplit = true;
    document.body.style.cursor = isHorizontal ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    e.preventDefault();
  });

  const onMouseMove = (e) => {
    if (!isResizingSplit) return;
    const rect = container.getBoundingClientRect();
    let pct;
    if (isHorizontal) {
      pct = Math.max(20, Math.min(80, (e.clientX - rect.left) / rect.width * 100));
      firstTab.container.style.width = `${pct}%`;
      secondTab.container.style.width = `${100 - pct}%`;
    } else {
      pct = Math.max(20, Math.min(80, (e.clientY - rect.top) / rect.height * 100));
      firstTab.container.style.height = `${pct}%`;
      secondTab.container.style.height = `${100 - pct}%`;
    }
  };

  const onMouseUp = () => {
    if (!isResizingSplit) return;
    isResizingSplit = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    resizeBar.style.background = '#333';
    firstTab.fitAddon.fit();
    secondTab.fitAddon.fit();
  };

  document.addEventListener('mousemove', onMouseMove);
  document.addEventListener('mouseup', onMouseUp);

  splitState = {
    direction,
    leftTabId: firstTab.id,
    rightTabId: secondTab.id,
    resizeEl: resizeBar,
    cleanupMove: onMouseMove,
    cleanupUp: onMouseUp,
  };

  // 리사이즈 + 포커스
  setTimeout(() => {
    leftTab.fitAddon.fit();
    rightTab.fitAddon.fit();
    rightTab.term.focus();
  }, 100);

  const dirLabel = isHorizontal ? '좌우' : '상하';
  showToast(`${dirLabel} 분할됨 (⌘D 해제)`);
}

// ── 탭 마우스 드래그 → 분할 ──

let splitDrag = null;

function initSplitDrag(tabId, startX, startY, tabEl) {
  // 이전 상태 정리
  cleanupSplitDrag();
  splitDrag = { tabId, startX, startY, tabEl, active: false, ghost: null, zones: [] };
}

document.addEventListener('mousemove', (e) => {
  if (!splitDrag) return;

  const dx = Math.abs(e.clientX - splitDrag.startX);
  const dy = Math.abs(e.clientY - splitDrag.startY);

  // 8px 이상 이동해야 드래그 시작
  if (!splitDrag.active) {
    if (dx < 8 && dy < 8) return;
    splitDrag.active = true;

    // 고스트 생성
    const tab = tabs.find(t => t.id === splitDrag.tabId);
    const ghost = document.createElement('div');
    ghost.className = 'drag-ghost';
    ghost.style.cssText = 'position:fixed;z-index:10001;padding:6px 14px;background:#0078d4;color:white;border-radius:6px;font-size:12px;pointer-events:none;opacity:0.9;white-space:nowrap;box-shadow:0 4px 12px rgba(0,0,0,0.3);';
    ghost.textContent = tab ? tab.title : 'Terminal';
    document.body.appendChild(ghost);
    splitDrag.ghost = ghost;

    // 터미널 영역 기준으로 드롭존 위치 계산
    const termContainer = document.getElementById('terminal-container');
    const termRect = termContainer.getBoundingClientRect();

    [
      { side: 'left', label: '← 왼쪽', css: `left:${termRect.left}px;top:${termRect.top}px;width:${termRect.width/2}px;height:${termRect.height}px;` },
      { side: 'right', label: '오른쪽 →', css: `left:${termRect.left + termRect.width/2}px;top:${termRect.top}px;width:${termRect.width/2}px;height:${termRect.height}px;` },
    ].forEach(({ side, label, css }) => {
      const el = document.createElement('div');
      el.className = 'drag-ghost-zone';
      el.style.cssText = `position:fixed;z-index:10000;display:flex;align-items:center;justify-content:center;font-size:15px;font-weight:600;color:rgba(255,255,255,0.3);background:rgba(0,0,0,0.15);pointer-events:none;${css}`;
      el.textContent = label;
      document.body.appendChild(el);
      splitDrag.zones.push({ side, el });
    });
  }

  // 고스트 이동
  if (splitDrag.ghost) {
    splitDrag.ghost.style.left = `${e.clientX + 10}px`;
    splitDrag.ghost.style.top = `${e.clientY + 10}px`;
  }

  // 드롭존 하이라이트 (마우스 위치로 판정)
  splitDrag.zones.forEach(z => {
    const rect = z.el.getBoundingClientRect();
    const hit = e.clientX >= rect.left && e.clientX <= rect.right &&
                e.clientY >= rect.top && e.clientY <= rect.bottom;
    z.el.style.background = hit ? 'rgba(0,120,212,0.4)' : 'rgba(0,0,0,0.15)';
    z.el.style.color = hit ? 'white' : 'rgba(255,255,255,0.3)';
    z.el.style.outline = hit ? '2px solid #0078d4' : 'none';
    z.el.style.outlineOffset = hit ? '-2px' : '';
  });
});

document.addEventListener('mouseup', (e) => {
  if (!splitDrag) return;

  const tabId = splitDrag.tabId;
  const wasActive = splitDrag.active;

  // 드롭존 히트 체크
  let hitSide = null;
  if (wasActive) {
    splitDrag.zones.forEach(z => {
      const rect = z.el.getBoundingClientRect();
      if (e.clientX >= rect.left && e.clientX <= rect.right &&
          e.clientY >= rect.top && e.clientY <= rect.bottom) {
        hitSide = z.side;
      }
    });
  }

  // 정리
  cleanupSplitDrag();
  splitDrag = null;

  // 분할 실행
  if (hitSide && !splitState) {
    const direction = hitSide === 'bottom' ? 'vertical' : 'horizontal';
    // 드래그한 탭이 활성 탭이면 다른 탭을 first로
    if (tabId === activeTabId) {
      const otherTab = tabs.find(t => t.id !== tabId);
      if (otherTab) {
        switchTab(otherTab.id);
        setTimeout(() => splitWithExisting(tabId, direction), 50);
      }
    } else {
      setTimeout(() => splitWithExisting(tabId, direction), 50);
    }
  }
});

function cleanupSplitDrag() {
  if (!splitDrag) return;
  if (splitDrag.ghost) splitDrag.ghost.remove();
  splitDrag.zones.forEach(z => z.el.remove());
  // body에 남은 잔여물도 제거
  document.querySelectorAll('.drag-ghost, .drag-ghost-zone').forEach(el => el.remove());
}

// ── 탭 우클릭 메뉴 ──

let tabMenuTargetId = null;

function showTabMenu(e, tabId) {
  tabMenuTargetId = tabId;
  const menu = document.getElementById('tab-context-menu');
  menu.classList.remove('hidden');
  const x = Math.min(e.clientX, window.innerWidth - 160);
  const y = Math.min(e.clientY, window.innerHeight - 120);
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
}

document.addEventListener('click', () => {
  document.getElementById('tab-context-menu')?.classList.add('hidden');
});

document.querySelectorAll('#tab-context-menu .context-item').forEach(item => {
  item.addEventListener('click', (e) => {
    e.stopPropagation();
    document.getElementById('tab-context-menu').classList.add('hidden');
    if (tabMenuTargetId === null) return;

    // 먼저 해당 탭으로 전환
    switchTab(tabMenuTargetId);

    switch (item.dataset.action) {
      case 'split-h':
        if (!splitState) toggleSplit('horizontal');
        break;
      case 'split-v':
        if (!splitState) toggleSplit('vertical');
        break;
      case 'close':
        closeTab(tabMenuTargetId);
        break;
    }
    tabMenuTargetId = null;
  });
});

// 기존 탭으로 분할 (새 탭 안 만듦)
function splitWithExisting(secondTabId, direction = 'horizontal') {
  const container = document.getElementById('terminal-container');
  if (splitState) return;

  const firstTab = tabs.find(t => t.id === activeTabId);
  const secondTab = tabs.find(t => t.id === secondTabId);
  if (!firstTab || !secondTab || firstTab.id === secondTab.id) return;

  const isHorizontal = direction === 'horizontal';

  container.style.display = 'flex';
  container.style.flexDirection = isHorizontal ? 'row' : 'column';

  // 다른 탭 숨기기
  tabs.forEach(t => {
    if (t.id !== firstTab.id && t.id !== secondTab.id) {
      t.container.style.display = 'none';
    }
  });

  firstTab.container.style.display = 'block';
  firstTab.container.style[isHorizontal ? 'width' : 'height'] = '50%';
  firstTab.container.style[isHorizontal ? 'height' : 'width'] = '100%';
  firstTab.container.style.flex = 'none';

  const resizeBar = document.createElement('div');
  resizeBar.id = 'split-resize-bar';
  if (isHorizontal) {
    resizeBar.style.cssText = 'width:4px;height:100%;cursor:col-resize;background:#333;flex-shrink:0;transition:background 0.15s;';
  } else {
    resizeBar.style.cssText = 'height:4px;width:100%;cursor:row-resize;background:#333;flex-shrink:0;transition:background 0.15s;';
  }
  let isResizing = false;
  resizeBar.addEventListener('mouseenter', () => resizeBar.style.background = '#0078d4');
  resizeBar.addEventListener('mouseleave', () => { if (!isResizing) resizeBar.style.background = '#333'; });

  secondTab.container.style.display = 'block';
  secondTab.container.style[isHorizontal ? 'width' : 'height'] = '50%';
  secondTab.container.style[isHorizontal ? 'height' : 'width'] = '100%';
  secondTab.container.style.flex = 'none';

  container.appendChild(resizeBar);
  container.appendChild(secondTab.container);

  resizeBar.addEventListener('mousedown', (e) => {
    isResizing = true;
    document.body.style.cursor = isHorizontal ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    e.preventDefault();
  });

  const onMove = (e) => {
    if (!isResizing) return;
    const rect = container.getBoundingClientRect();
    let pct;
    if (isHorizontal) {
      pct = Math.max(20, Math.min(80, (e.clientX - rect.left) / rect.width * 100));
      firstTab.container.style.width = `${pct}%`;
      secondTab.container.style.width = `${100 - pct}%`;
    } else {
      pct = Math.max(20, Math.min(80, (e.clientY - rect.top) / rect.height * 100));
      firstTab.container.style.height = `${pct}%`;
      secondTab.container.style.height = `${100 - pct}%`;
    }
  };

  const onUp = () => {
    if (!isResizing) return;
    isResizing = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    resizeBar.style.background = '#333';
    firstTab.fitAddon.fit();
    secondTab.fitAddon.fit();
  };

  document.addEventListener('mousemove', onMove);
  document.addEventListener('mouseup', onUp);

  splitState = {
    direction,
    leftTabId: firstTab.id,
    rightTabId: secondTab.id,
    resizeEl: resizeBar,
    cleanupMove: onMove,
    cleanupUp: onUp,
  };

  setTimeout(() => {
    firstTab.fitAddon.fit();
    secondTab.fitAddon.fit();
    secondTab.term.focus();
  }, 100);

  showToast(`${isHorizontal ? '좌우' : '상하'} 분할됨 (⌘D 해제)`);
}

function focusSplitPane(side) {
  if (!splitState) return;
  const tabId = side === 'left' ? splitState.leftTabId : splitState.rightTabId;
  const tab = tabs.find(t => t.id === tabId);
  if (tab) {
    tab.term.focus();
    activeTabId = tabId;
    renderTabBar();
  }
}

function unsplit() {
  if (!splitState) return;
  const container = document.getElementById('terminal-container');

  // 이벤트 리스너 제거
  document.removeEventListener('mousemove', splitState.cleanupMove);
  document.removeEventListener('mouseup', splitState.cleanupUp);

  // 리사이즈 바 전부 제거
  container.querySelectorAll('#split-resize-bar').forEach(el => el.remove());

  // 컨테이너 인라인 스타일 완전 초기화
  container.removeAttribute('style');

  // 모든 탭 인라인 스타일 완전 초기화 + 활성 탭만 표시
  tabs.forEach(t => {
    t.container.removeAttribute('style');
    t.container.style.display = t.id === activeTabId ? 'block' : 'none';
    t.container.style.width = '100%';
    t.container.style.height = '100%';
  });

  splitState = null;

  // 활성 탭 리사이즈
  setTimeout(() => {
    const tab = tabs.find(t => t.id === activeTabId);
    if (tab) {
      tab.fitAddon.fit();
      tab.term.focus();
    }
  }, 50);

  showToast('분할 해제');
}

// ── 검색 ──

function toggleSearch() {
  const bar = document.getElementById('search-bar');
  const isHidden = bar.classList.contains('hidden');
  bar.classList.toggle('hidden');
  if (isHidden) {
    const input = document.getElementById('search-input');
    input.focus();
    input.select();
  } else {
    // 검색 닫기 → 터미널 포커스
    const tab = tabs.find(t => t.id === activeTabId);
    if (tab) tab.term.focus();
  }
}

document.getElementById('search-input').addEventListener('input', (e) => {
  const tab = tabs.find(t => t.id === activeTabId);
  if (tab && e.target.value) {
    tab.searchAddon.findNext(e.target.value);
  }
});

document.getElementById('search-input').addEventListener('keydown', (e) => {
  const tab = tabs.find(t => t.id === activeTabId);
  if (!tab) return;
  if (e.key === 'Enter') {
    e.preventDefault();
    if (e.shiftKey) {
      tab.searchAddon.findPrevious(e.target.value);
    } else {
      tab.searchAddon.findNext(e.target.value);
    }
  }
  if (e.key === 'Escape') {
    toggleSearch();
  }
});

document.getElementById('search-next').addEventListener('click', () => {
  const tab = tabs.find(t => t.id === activeTabId);
  const val = document.getElementById('search-input').value;
  if (tab && val) tab.searchAddon.findNext(val);
});

document.getElementById('search-prev').addEventListener('click', () => {
  const tab = tabs.find(t => t.id === activeTabId);
  const val = document.getElementById('search-input').value;
  if (tab && val) tab.searchAddon.findPrevious(val);
});

document.getElementById('search-close').addEventListener('click', toggleSearch);

// ── 컨텍스트 메뉴 ──

let contextTarget = null; // { tabId, type, path, name, isDir, cwd }

function showContextMenu(e, target) {
  e.preventDefault();
  contextTarget = target;
  const menu = document.getElementById('context-menu');
  menu.classList.remove('hidden');

  // 위치 조정 (화면 밖으로 안 나가게)
  const x = Math.min(e.clientX, window.innerWidth - 180);
  const y = Math.min(e.clientY, window.innerHeight - 150);
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;

  // 폴더면 다운로드 숨기기
  const dlBtn = menu.querySelector('[data-action="download"]');
  dlBtn.style.display = target.isDir ? 'none' : 'block';

  // Sublime 편집은 원격(SSH) 파일에만 노출
  const editBtn = menu.querySelector('[data-action="edit-sublime"]');
  if (editBtn) editBtn.style.display = (target.type === 'ssh' && !target.isDir) ? 'block' : 'none';
}

function hideContextMenu() {
  document.getElementById('context-menu').classList.add('hidden');
  contextTarget = null;
}

// 클릭하면 메뉴 닫기
document.addEventListener('click', hideContextMenu);

// 컨텍스트 메뉴 액션
document.querySelectorAll('.context-item').forEach(item => {
  item.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (!contextTarget) return;
    const { tabId, type, path, name, isDir, cwd } = contextTarget;
    hideContextMenu();

    switch (item.dataset.action) {
      case 'edit-sublime':
        try {
          await invoke('sftp_edit', { id: tabId, remotePath: path });
          showToast(`Sublime으로 편집: ${name} (저장하면 서버에 자동 반영)`);
        } catch (e) {
          showToast(`편집 열기 실패: ${e}`, true);
        }
        break;

      case 'download':
        downloadFile(tabId, path, name);
        break;

      case 'rename': {
        const newName = prompt('새 이름:', name);
        if (!newName || newName === name) break;
        const dir = path.substring(0, path.lastIndexOf('/'));
        const newPath = `${dir}/${newName}`;
        try {
          if (type === 'ssh') {
            await invoke('sftp_rename', { id: tabId, oldPath: path, newPath });
            loadRemoteFiles(tabId, cwd);
          } else {
            await invoke('local_rename', { oldPath: path, newPath });
            loadLocalFiles(tabId, cwd);
          }
          showToast(`이름 변경: ${newName}`);
        } catch (e) {
          showToast(`이름 변경 실패: ${e}`, true);
        }
        break;
      }

      case 'delete': {
        if (!confirm(`"${name}" 를 삭제하시겠습니까?`)) break;
        try {
          if (type === 'ssh') {
            await invoke('sftp_delete', { id: tabId, remotePath: path, isDir });
            loadRemoteFiles(tabId, cwd);
          } else {
            await invoke('local_delete', { path, isDir });
            loadLocalFiles(tabId, cwd);
          }
          showToast(`삭제됨: ${name}`);
        } catch (e) {
          showToast(`삭제 실패: ${e}`, true);
        }
        break;
      }

      case 'copy-path': {
        try {
          await navigator.clipboard.writeText(path);
          showToast('경로 복사됨');
        } catch {
          showToast(path);
        }
        break;
      }
    }
  });
});

// ── 사이드바 리사이즈 ──

const resizeHandle = document.getElementById('sidebar-resize');
const sidebar = document.getElementById('sidebar');
let isResizing = false;

resizeHandle.addEventListener('mousedown', (e) => {
  isResizing = true;
  resizeHandle.classList.add('active');
  document.body.style.cursor = 'col-resize';
  document.body.style.userSelect = 'none';
  e.preventDefault();
});

document.addEventListener('mousemove', (e) => {
  if (!isResizing) return;
  const newWidth = Math.max(150, Math.min(500, e.clientX));
  sidebar.style.width = `${newWidth}px`;
  sidebar.style.minWidth = `${newWidth}px`;
  // 터미널 리사이즈
  const activeTab = tabs.find(t => t.id === activeTabId);
  if (activeTab) activeTab.fitAddon.fit();
});

document.addEventListener('mouseup', () => {
  if (isResizing) {
    isResizing = false;
    resizeHandle.classList.remove('active');
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    // 터미널 리사이즈 확정
    const activeTab = tabs.find(t => t.id === activeTabId);
    if (activeTab) activeTab.fitAddon.fit();
  }
});

// ── 초기화 ──

// 사이드바 기본 열림 상태 반영
document.getElementById('sidebar-toggle-btn').classList.add('active');

// 저장된 설정 로드
(async () => {
  try {
    const settings = await invoke('load_app_settings');
    if (settings.theme) {
      const found = THEMES.find(t => t.name === settings.theme);
      if (found) {
        currentTheme = found;
        applyUiTheme(found);
      }
    }
    if (settings.fontSize) currentFontSize = settings.fontSize;
    if (settings.cursorStyle) currentCursorStyle = settings.cursorStyle;
    if (settings.showHiddenFiles !== undefined) showHiddenFiles = settings.showHiddenFiles;
  } catch (e) {
    console.log('No saved settings:', e);
  }

  createTab();
})();

// SSH 서버 목록 초기 로드
loadSshList();
