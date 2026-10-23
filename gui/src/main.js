/* Paranoia 解密工具 — 前端逻辑（纯静态 JS，无打包步骤）
 * 依赖 window.__TAURI__（tauri.conf.json: withGlobalTauri = true）
 */
(() => {
  'use strict';

  /* ------------------------------------------------------------------ */
  /* Tauri API 句柄（找不到时给出友好提示，便于浏览器里预览样式）          */
  /* ------------------------------------------------------------------ */
  const TAURI = window.__TAURI__ || {};
  const invoke = TAURI.core && typeof TAURI.core.invoke === 'function' ? TAURI.core.invoke : null;
  const listen = TAURI.event && typeof TAURI.event.listen === 'function' ? TAURI.event.listen : null;
  const getCurrentWebview =
    (TAURI.webview && typeof TAURI.webview.getCurrentWebview === 'function' && TAURI.webview.getCurrentWebview) ||
    (TAURI.window && typeof TAURI.window.getCurrentWebview === 'function' && TAURI.window.getCurrentWebview) ||
    null;

  /* ------------------------------------------------------------------ */
  /* 错误码 -> 中文提示                                                  */
  /* ------------------------------------------------------------------ */
  const ERROR_TEXT = {
    wrongPassword: '密码错误',
    integrity: '文件损坏或已被篡改',
    truncated: '文件被截断',
    tooShort: '文件过短',
    badMagic: '不是 Paranoia 加密文件',
    unsupportedVersion: '不支持的格式版本',
    unsupportedAlgorithm: '不是 AES-256 加密文件',
    invalidCustomParams: '不支持的 KDF 参数',
    unsupportedRecovery: '包含暂不支持的恢复数据',
    zip: '压缩数据损坏',
    invalidOutputName: '压缩包内文件名非法',
    outputExists: '输出文件已存在',
    io: '读写错误',
    crypto: '内部错误'
  };

  /* 添加文件时被拒绝的原因（AddFileError::kind + GUI 侧前置检查） */
  const ADD_REASON_TEXT = {
    notEncExtension: '不是 .enc 文件',
    notEnc: '不是 .enc 文件',
    notFound: '文件不存在',
    notFile: '不是普通文件',
    cannotRead: '无法读取文件',
    tooShort: '文件过短',
    badMagic: '不是 Paranoia 加密文件',
    unsupportedVersion: '不支持的格式版本',
    unsupportedAlgorithm: '不是 AES-256 加密文件',
    invalidCustomParams: '不支持的 KDF 参数',
    emptyPath: '路径为空'
  };

  const CONFLICT_HINTS = {
    rename: '自动追加 (1)、(2) 等后缀，绝不覆盖已有文件',
    overwrite: '直接覆盖输出目录中的同名文件',
    error: '遇到同名文件时该文件解密失败'
  };

  /* ------------------------------------------------------------------ */
  /* 全局状态                                                            */
  /* ------------------------------------------------------------------ */
  const state = {
    files: [],              // [{ path, canonicalPath, size, name, dir }]
    customOutDir: null,     // 用户选择的目录；null = 使用默认（首个文件所在目录）
    defaultOutDir: null,    // 由后端计算的默认目录（展示用）
    running: false,
    idleOverallLabel: '等待开始',
    rowRefs: [],            // 与 files 等长的 DOM 句柄
    rowsByKey: new Map(),   // normalizeKey(path) -> row 句柄
    outDirToken: 0,
    openOutputDir: null,
    lastSummary: null,
    fallbackRowIdx: 0,
    dragDropBound: false,
    dragDropEvents: [],
    dragDropApi: !!getCurrentWebview,
    dragDropError: null,
    toastTimer: 0,
    configPath: null,
    initialRememberedPassword: null,
    initialAutotest: null,
    initialStartupMs: null,
    auto: null              // 自动验证（仅 --autotest 启动时存在）
  };

  const els = {};

  function cacheEls() {
    const ids = [
      'app', 'statusHint', 'statusDot',
      'fileCount', 'btnClear', 'btnAdd', 'dropzone', 'fileList', 'emptyHint', 'fileRowTemplate',
      'passwordInput', 'btnTogglePassword', 'rememberPassword', 'btnClearPassword', 'configHint',
      'outputDirDisplay', 'btnChooseOut', 'btnResetOut', 'outputHint',
      'conflictSelect', 'conflictHint',
      'overallLabel', 'overallPct', 'overallFill', 'statOk', 'statFail', 'statTotal', 'btnOpenOut',
      'btnStart', 'toast'
    ];
    for (const id of ids) els[id] = document.getElementById(id);
    els.app = document.querySelector('.app');
  }

  /* ------------------------------------------------------------------ */
  /* 小工具                                                              */
  /* ------------------------------------------------------------------ */
  const normalizeKey = (p) => String(p || '').replace(/\/+$/, '').toLowerCase();
  const basename = (p) => {
    const parts = String(p || '').split('/');
    return parts[parts.length - 1] || String(p || '');
  };
  const dirname = (p) => {
    const s = String(p || '');
    const i = s.lastIndexOf('/');
    return i > 0 ? s.slice(0, i) : '/';
  };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  function formatBytes(n) {
    const v = Number(n);
    if (!Number.isFinite(v) || v < 0) return '—';
    if (v < 1024) return v + ' B';
    const units = ['KB', 'MB', 'GB', 'TB'];
    let x = v / 1024;
    let i = 0;
    while (x >= 1024 && i < units.length - 1) { x /= 1024; i += 1; }
    return (x >= 100 ? x.toFixed(0) : x >= 10 ? x.toFixed(1) : x.toFixed(2)) + ' ' + units[i];
  }

  function errText(err) {
    if (err == null) return '未知错误';
    if (typeof err === 'string') return err;
    if (typeof err === 'number') return String(err);
    if (err.message) return String(err.message);
    try { return JSON.stringify(err); } catch { return String(err); }
  }

  function errorTextFor(kind, fallback) {
    if (kind && ERROR_TEXT[kind]) return ERROR_TEXT[kind];
    if (fallback) return String(fallback);
    return kind ? String(kind) : '未知错误';
  }

  function addReasonText(v) {
    const kind = v && v.reason;
    if (kind && ADD_REASON_TEXT[kind]) return ADD_REASON_TEXT[kind];
    if (kind && ERROR_TEXT[kind]) return ERROR_TEXT[kind];
    return (v && (v.reasonMessage || v.reason)) || '未知原因';
  }

  function showToast(message, type, duration) {
    const el = els.toast;
    if (!el) return;
    el.textContent = message;
    el.className = 'toast is-visible' + (type ? ' is-' + type : '');
    el.hidden = false;
    if (state.toastTimer) clearTimeout(state.toastTimer);
    state.toastTimer = window.setTimeout(() => {
      el.classList.remove('is-visible');
      state.toastTimer = window.setTimeout(() => { el.hidden = true; }, 220);
    }, duration || 3600);
  }

  function setStatus(text, mode) {
    if (els.statusHint) els.statusHint.textContent = text;
    if (els.app) {
      els.app.classList.toggle('is-busy', mode === 'busy');
      els.app.classList.toggle('has-error', mode === 'error');
    }
  }

  function setDropActive(active) {
    if (els.dropzone) els.dropzone.classList.toggle('is-drag', !!active);
  }

  /* ------------------------------------------------------------------ */
  /* 文件列表渲染                                                        */
  /* ------------------------------------------------------------------ */
  function renderFileList() {
    if (!els.fileList) return;
    els.fileList.replaceChildren();
    state.rowRefs = [];
    state.rowsByKey.clear();

    const template = els.fileRowTemplate;
    state.files.forEach((file, index) => {
      const node = template.content.firstElementChild.cloneNode(true);
      node.dataset.path = file.path;
      node.dataset.index = String(index);
      node.querySelector('.file-name').textContent = file.name;
      node.querySelector('.file-name').title = file.path;
      node.querySelector('.file-path').textContent = file.dir;
      node.querySelector('.file-size').textContent = formatBytes(file.size);
      if (state.running) node.classList.add('is-locked');

      const row = {
        file,
        index,
        node,
        status: node.querySelector('.file-status'),
        fill: node.querySelector('.file-fill'),
        badge: node.querySelector('.file-badge')
      };
      state.rowRefs.push(row);
      state.rowsByKey.set(normalizeKey(file.path), row);
      els.fileList.appendChild(node);
    });

    if (els.emptyHint) els.emptyHint.hidden = state.files.length > 0;
    if (els.fileCount) els.fileCount.textContent = String(state.files.length);
    updateStartEnabled();
    updateStats(0, 0);
  }

  function rowForInput(input) {
    const row = state.rowsByKey.get(normalizeKey(input));
    if (row) return row;
    while (state.fallbackRowIdx < state.rowRefs.length) {
      const candidate = state.rowRefs[state.fallbackRowIdx];
      state.fallbackRowIdx += 1;
      return candidate;
    }
    return null;
  }

  function resetRow(row) {
    if (!row) return;
    row.node.classList.remove('is-running', 'is-ok', 'is-fail');
    row.status.textContent = '等待中';
    row.status.title = '';
    row.fill.style.width = '0%';
    row.badge.textContent = '';
  }

  function resetAllRows() {
    state.fallbackRowIdx = 0;
    state.rowRefs.forEach(resetRow);
  }

  function setRowState(row, kind, text, title, pct) {
    if (!row) return;
    row.node.classList.remove('is-running', 'is-ok', 'is-fail');
    if (kind) row.node.classList.add('is-' + kind);
    row.status.textContent = text;
    row.status.title = title || '';
    if (typeof pct === 'number') row.fill.style.width = Math.max(0, Math.min(100, pct)) + '%';
  }

  function applyOutcome(row, outcome) {
    if (!row || !outcome) return;
    if (outcome.ok) {
      const outs = Array.isArray(outcome.outputs) ? outcome.outputs : [];
      const extra = outcome.originalName ? ' → ' + outcome.originalName : '';
      setRowState(row, 'ok', '成功', outs.join('\n') + extra, 100);
      if (outcome.originalName && outcome.originalName !== '') row.badge.textContent = outcome.originalName;
    } else {
      const text = errorTextFor(outcome.errorKind, '失败');
      setRowState(row, 'fail', text, outcome.errorMessage || outcome.errorKind || '解密失败', 100);
      row.badge.textContent = '失败';
    }
  }

  function removeFile(path) {
    if (state.running) return;
    const key = normalizeKey(path);
    const idx = state.files.findIndex((f) => normalizeKey(f.path) === key);
    if (idx < 0) return;
    state.files.splice(idx, 1);
    renderFileList();
    refreshDefaultOutDir();
    if (!state.files.length) {
      setOverall(0, state.idleOverallLabel, false);
      setStatus('就绪');
    }
  }

  function clearFiles() {
    if (state.running) return;
    if (!state.files.length) {
      showToast('文件列表已为空');
      return;
    }
    state.files = [];
    renderFileList();
    refreshDefaultOutDir();
    setOverall(0, state.idleOverallLabel, false);
    setStatus('就绪');
    showToast('已清空文件列表');
  }

  function updateStartEnabled() {
    if (!els.btnStart) return;
    els.btnStart.disabled = state.running || !state.files.length;
    els.btnStart.classList.toggle('is-running', state.running);
    const label = els.btnStart.querySelector('.start-label');
    if (label) label.textContent = state.running ? '解密中…' : '开始解密';
  }

  function setUiRunning(running) {
    state.running = running;
    updateStartEnabled();
    const lock = running ? 'is-locked' : '';
    const controls = [els.btnAdd, els.btnClear, els.btnChooseOut, els.btnResetOut, els.passwordInput,
      els.btnTogglePassword, els.btnClearPassword, els.rememberPassword, els.conflictSelect];
    for (const el of controls) {
      if (!el) continue;
      el.disabled = running;
    }
    state.rowRefs.forEach((row) => row.node.classList.toggle('is-locked', running));
    if (els.dropzone) els.dropzone.style.pointerEvents = running ? 'none' : '';
    if (els.btnResetOut) els.btnResetOut.disabled = running || !state.customOutDir;
    if (!running && els.btnClear) els.btnClear.disabled = false;
    setStatus(running ? '正在解密…' : '就绪', running ? 'busy' : '');
    void lock;
  }

  /* ------------------------------------------------------------------ */
  /* 密码 / 输出目录 / 冲突策略                                          */
  /* ------------------------------------------------------------------ */
  function setPasswordVisibility(visible) {
    els.passwordInput.type = visible ? 'text' : 'password';
    els.btnTogglePassword.classList.toggle('is-visible', visible);
    els.btnTogglePassword.title = visible ? '隐藏密码' : '显示密码';
  }

  function updateConfigHint(initial) {
    const path = initial && initial.configPath;
    if (!els.configHint) return;
    if (path) {
      els.configHint.textContent = '明文保存于 ' + path;
      els.configHint.title = '密码以明文形式保存在本机配置文件：' + path;
    } else {
      els.configHint.textContent = '密码以明文保存在本机配置文件中';
      els.configHint.title = '密码以明文形式保存在本机配置目录';
    }
  }

  async function saveRememberedPassword() {
    if (!invoke) return;
    try {
      await invoke('set_remembered_password', { password: els.passwordInput.value });
    } catch (err) {
      showToast('保存记忆密码失败：' + errText(err), 'warn');
    }
  }

  async function clearRememberedPassword() {
    if (!invoke) return;
    try {
      await invoke('clear_remembered_password');
      els.rememberPassword.checked = false;
      showToast('已清除本机保存的密码');
    } catch (err) {
      showToast('清除记忆密码失败：' + errText(err), 'error');
    }
  }

  function displayPath(path) {
    if (!els.outputDirDisplay) return;
    const span = document.createElement('span');
    span.className = 'path-text';
    span.textContent = path;
    els.outputDirDisplay.replaceChildren(span);
    els.outputDirDisplay.title = path;
  }

  function customSet() {
    return !!state.customOutDir;
  }

  function updateOutDirDisplay() {
    if (!els.outputDirDisplay) return;
    if (customSet()) {
      displayPath(state.customOutDir);
      els.outputDirDisplay.classList.add('is-custom');
      els.outputHint.textContent = '自定义输出目录（覆盖默认）';
      els.btnResetOut.disabled = state.running;
    } else {
      els.outputDirDisplay.classList.remove('is-custom');
      if (state.defaultOutDir) {
        displayPath(state.defaultOutDir);
        els.outputHint.textContent = '默认：首个文件所在目录';
      } else if (state.files.length) {
        displayPath('（正在计算默认目录…）');
        els.outputHint.textContent = '默认：首个文件所在目录';
      } else {
        displayPath('（尚未添加文件）');
        els.outputHint.textContent = '默认：首个文件所在目录';
      }
      els.btnResetOut.disabled = true;
    }
  }

  async function refreshDefaultOutDir() {
    if (customSet()) { updateOutDirDisplay(); return; }
    if (!state.files.length || !invoke) {
      state.defaultOutDir = null;
      updateOutDirDisplay();
      return;
    }
    const token = ++state.outDirToken;
    try {
      const dir = await invoke('get_default_output_dir', { paths: state.files.map((f) => f.path) });
      if (token !== state.outDirToken) return;
      state.defaultOutDir = dir || null;
    } catch (err) {
      if (token !== state.outDirToken) return;
      state.defaultOutDir = null;
    }
    updateOutDirDisplay();
  }

  async function chooseOutputDir() {
    if (!invoke || state.running) return;
    try {
      const dir = await invoke('pick_output_dir', { current: state.customOutDir });
      if (dir) {
        state.customOutDir = dir;
        updateOutDirDisplay();
        showToast('输出目录：' + dir);
      }
    } catch (err) {
      showToast('选择输出目录失败：' + errText(err), 'error');
    }
  }

  function resetOutputDir() {
    if (state.running || !customSet()) return;
    state.customOutDir = null;
    state.defaultOutDir = null;
    updateOutDirDisplay();
    refreshDefaultOutDir();
    showToast('已恢复默认输出目录');
  }

  function updateConflictHint() {
    const value = els.conflictSelect.value;
    els.conflictHint.textContent = CONFLICT_HINTS[value] || CONFLICT_HINTS.rename;
  }

  /* ------------------------------------------------------------------ */
  /* 进度与统计                                                          */
  /* ------------------------------------------------------------------ */
  function setOverall(pct, label, done) {
    const safe = Math.max(0, Math.min(100, Number(pct) || 0));
    if (els.overallFill) {
      els.overallFill.style.width = safe + '%';
      els.overallFill.classList.toggle('is-done', !!done);
    }
    if (els.overallPct) els.overallPct.textContent = safe >= 100 ? '100%' : safe.toFixed(0) + '%';
    if (label != null && els.overallLabel) els.overallLabel.textContent = label;
  }

  function updateStats(ok, fail, totalFiles) {
    if (els.statOk) els.statOk.textContent = String(ok || 0);
    if (els.statFail) els.statFail.textContent = String(fail || 0);
    if (els.statTotal) {
      const total = typeof totalFiles === 'number' ? totalFiles : state.files.length;
      const done = (ok || 0) + (fail || 0);
      els.statTotal.textContent = done + ' / ' + total + ' 个文件';
    }
  }

  function resetProgressUi() {
    state.fallbackRowIdx = 0;
    resetAllRows();
    setOverall(0, '准备开始…', false);
    updateStats(0, 0);
    state.openOutputDir = null;
    if (els.btnOpenOut) els.btnOpenOut.hidden = true;
  }

  /* ------------------------------------------------------------------ */
  /* 后端事件                                                            */
  /* ------------------------------------------------------------------ */
  function onBatchProgress(payload) {
    if (!payload) return;
    const total = Number(payload.total) || state.files.length;
    const index = Number(payload.index) || 0;
    const overallTotal = Number(payload.overallTotal) || 0;
    const overallProcessed = Number(payload.overallProcessed) || 0;
    let pct = overallTotal > 0 ? (overallProcessed / overallTotal) * 100 : (total > 0 ? (index / total) * 100 : 0);
    if (pct > 99.4) pct = Math.min(99, pct);
    const name = payload.currentFile ? basename(payload.currentFile) : '';
    setOverall(pct, '正在解密 ' + (index + 1) + '/' + total + (name ? ' · ' + name : ''));
    updateStats(payload.successes, payload.failures, total);

    if (state.auto) {
      state.auto.progressCount += 1;
      state.auto.lastProgress = payload;
    }

    const row = state.rowRefs[Math.min(Math.max(index, 0), Math.max(state.rowRefs.length - 1, 0))];
    // 已经结束（成功/失败）的行不再被"解密中"覆盖；最终结果由 batch-finished 汇总校准。
    const rowDone = row && (row.node.classList.contains('is-ok') || row.node.classList.contains('is-fail'));
    if (row && !rowDone) {
      const fileTotal = Number(payload.fileTotal) || 0;
      const fileProcessed = Number(payload.fileProcessed) || 0;
      const fpct = fileTotal > 0 ? Math.min(100, (fileProcessed / fileTotal) * 100) : 0;
      row.node.classList.add('is-running');
      row.status.textContent = '解密中 ' + fpct.toFixed(0) + '%';
      row.status.title = '';
      row.fill.style.width = fpct + '%';
    }
  }

  function onFileFinished(payload) {
    if (!payload) return;
    if (state.auto) state.auto.fileFinishedCount += 1;
    const row = rowForInput(payload.input);
    if (!row) return;
    if (!payload.ok) {
      setRowState(row, 'fail', payload.errorKind ? errorTextFor(payload.errorKind) : '失败',
        payload.errorMessage || payload.errorKind || '解密失败', 100);
      row.badge.textContent = '失败';
      return;
    }
    const outs = Array.isArray(payload.outputs) ? payload.outputs : [];
    if (outs.length) {
      applyOutcome(row, payload);
    } else {
      // 运行中的阶段性事件（尚无输出信息），最终结果由 batch-finished 覆盖
      const fileTotal = row.fill && row.status ? 0 : 0;
      void fileTotal;
      setRowState(row, 'ok', '成功', '', 100);
    }
  }

  function onBatchFinished(summary) {
    if (!summary) return;
    state.lastSummary = summary;
    state.running = false;
    setUiRunning(false);

    const outcomes = Array.isArray(summary.outcomes) ? summary.outcomes : [];
    state.fallbackRowIdx = 0;
    outcomes.forEach((outcome) => {
      const row = rowForInput(outcome.input);
      applyOutcome(row, outcome);
    });
    // 汇总里没有的行（理论上不会出现）标记为未处理
    state.rowRefs.forEach((row) => {
      if (!row.node.classList.contains('is-ok') && !row.node.classList.contains('is-fail')) {
        setRowState(row, 'fail', '未处理', '未返回结果', 100);
      }
    });

    const ok = Number(summary.successes) || 0;
    const fail = Number(summary.failures) || 0;
    updateStats(ok, fail, state.files.length);
    setOverall(100, '完成 · 成功 ' + ok + ' 个 · 失败 ' + fail + ' 个', true);
    setStatus(fail ? '完成（有失败文件）' : '全部成功', fail ? 'error' : '');

    if (summary.outputDir) {
      state.openOutputDir = summary.outputDir;
      if (els.btnOpenOut) els.btnOpenOut.hidden = false;
    }
    showToast(
      '解密完成：成功 ' + ok + ' 个，失败 ' + fail + ' 个' +
      (summary.outputDir ? ' · 输出到 ' + summary.outputDir : ''),
      fail ? 'warn' : 'success',
      5200
    );

    if (state.auto && state.auto.resolveBatchDone) {
      const resolve = state.auto.resolveBatchDone;
      state.auto.resolveBatchDone = null;
      resolve();
    }
  }

  async function bindTauriEvents() {
    if (!invoke) return;
    if (!listen) return;
    try {
      await listen('batch-progress', (event) => onBatchProgress(event && event.payload));
      await listen('file-finished', (event) => onFileFinished(event && event.payload));
      await listen('batch-finished', (event) => onBatchFinished(event && event.payload));
    } catch (err) {
      showToast('初始化事件监听失败：' + errText(err), 'error');
    }
  }

  /* ------------------------------------------------------------------ */
  /* 拖拽添加（Tauri v2 onDragDropEvent）                                 */
  /* ------------------------------------------------------------------ */
  /** Tauri v2 onDragDropEvent 的载荷：{ type: 'enter' | 'over' | 'drop' | 'leave', paths, position } */
  function handleDragDropPayload(rawPayload) {
    const payload = rawPayload || {};
    const type = payload.type;
    if (type === 'enter' || type === 'over') {
      setDropActive(true);
      return;
    }
    if (type === 'leave') {
      setDropActive(false);
      return;
    }
    if (type === 'drop') {
      setDropActive(false);
      const paths = Array.isArray(payload.paths) ? payload.paths : [];
      if (paths.length) addPaths(paths);
    }
  }

  async function bindDragDrop() {
    if (!getCurrentWebview) {
      state.dragDropError = 'window.__TAURI__.webview.getCurrentWebview 不可用';
      bindDomDragFallback();
      return;
    }
    try {
      const webview = getCurrentWebview();
      await webview.onDragDropEvent((event) => {
        const payload = (event && event.payload) || event || {};
        state.dragDropEvents.push({
          type: payload.type || null,
          paths: Array.isArray(payload.paths) ? payload.paths.slice(0, 8) : [],
          positions: payload.position ? [payload.position.x, payload.position.y] : null,
          at: Date.now()
        });
        if (state.dragDropEvents.length > 60) state.dragDropEvents.shift();
        handleDragDropPayload(payload);
      });
      state.dragDropBound = true;
      // 自动验证模式下的拖拽逻辑钩子（原生 OS 拖拽无法脚本化时，用同一处理函数验证载荷分支）
      window.__PARANOIA_AUTOTEST_DRAG__ = handleDragDropPayload;
    } catch (err) {
      state.dragDropError = errText(err);
      bindDomDragFallback();
    }
  }

  function bindDomDragFallback() {
    const stop = (e) => { e.preventDefault(); e.stopPropagation(); };
    document.addEventListener('dragenter', (e) => { stop(e); setDropActive(true); });
    document.addEventListener('dragover', (e) => { stop(e); setDropActive(true); });
    document.addEventListener('dragleave', (e) => {
      stop(e);
      if (!e.relatedTarget) setDropActive(false);
    });
    document.addEventListener('drop', (e) => {
      stop(e);
      setDropActive(false);
      const files = e.dataTransfer && e.dataTransfer.files ? Array.from(e.dataTransfer.files) : [];
      const paths = files.map((f) => f.path).filter(Boolean);
      if (paths.length) addPaths(paths);
    });
  }

  /* ------------------------------------------------------------------ */
  /* 添加/校验文件                                                       */
  /* ------------------------------------------------------------------ */
  async function addFilesFromDialog() {
    if (!invoke || state.running) return;
    try {
      const paths = await invoke('pick_input_files');
      if (Array.isArray(paths) && paths.length) await addPaths(paths);
    } catch (err) {
      showToast('打开文件选择窗口失败：' + errText(err), 'error');
    }
  }

  async function addPaths(paths) {
    if (!invoke || !Array.isArray(paths) || !paths.length) return;
    const unique = [];
    const seen = new Set();
    for (const p of paths) {
      if (typeof p !== 'string' || !p) continue;
      const key = normalizeKey(p);
      if (seen.has(key)) continue;
      seen.add(key);
      unique.push(p);
    }
    if (!unique.length) return;

    let validations;
    try {
      validations = await invoke('validate_paths', { paths: unique });
    } catch (err) {
      showToast('文件校验失败：' + errText(err), 'error');
      return;
    }
    if (!Array.isArray(validations)) return;

    const before = state.files.length;
    const rejected = [];
    let duplicates = 0;
    for (const v of validations) {
      if (!v) continue;
      if (!v.accepted) { rejected.push(v); continue; }
      const key = normalizeKey(v.canonicalPath || v.path);
      const already = state.files.some((f) => normalizeKey(f.canonicalPath) === key);
      if (already) { duplicates += 1; continue; }
      state.files.push({
        path: v.path,
        canonicalPath: v.canonicalPath || v.path,
        size: Number(v.size) || 0,
        name: v.fileName || basename(v.path),
        dir: dirname(v.path)
      });
    }
    if (state.files.length !== before) {
      renderFileList();
      refreshDefaultOutDir();
    }
    if (state.auto) state.auto.addValidations = state.auto.addValidations || [];
    if (state.auto) state.auto.addValidations.push({ requested: unique.length, accepted: state.files.length - before, rejected, duplicates });

    const added = state.files.length - before;
    const messages = [];
    if (added > 0) messages.push('已添加 ' + added + ' 个文件');
    if (duplicates > 0) messages.push('跳过 ' + duplicates + ' 个重复文件');
    if (rejected.length > 0) {
      const reasons = [];
      for (const v of rejected) {
        const text = addReasonText(v);
        if (!reasons.includes(text)) reasons.push(text);
      }
      messages.push('拒绝 ' + rejected.length + ' 个（' + reasons.slice(0, 3).join('、') + (reasons.length > 3 ? ' 等' : '') + '）');
    }
    if (messages.length) showToast(messages.join(' · '), rejected.length ? 'warn' : 'info');
    if (added === 0 && rejected.length === 0 && duplicates > 0) showToast('文件已在列表中');
  }

  function collectDroppedPaths() {
    return [];
  }

  /* ------------------------------------------------------------------ */
  /* 开始解密                                                            */
  /* ------------------------------------------------------------------ */
  async function startBatch() {
    if (!invoke || state.running) return;
    if (!state.files.length) {
      showToast('请先添加要解密的文件', 'warn');
      return;
    }
    const password = els.passwordInput.value;
    if (!password) {
      showToast('请输入解密密码', 'warn');
      els.passwordInput.focus();
      return;
    }

    resetProgressUi();
    state.lastSummary = null;
    state.running = true;
    setUiRunning(true);
    setOverall(0, '准备开始…');
    setStatus('正在解密…', 'busy');

    const request = {
      files: state.files.map((f) => f.path),
      password,
      outputDir: state.customOutDir,
      conflict: els.conflictSelect.value,
      rememberPassword: !!els.rememberPassword.checked
    };

    try {
      await invoke('start_batch', { request });
    } catch (err) {
      state.running = false;
      setUiRunning(false);
      setStatus('就绪');
      setOverall(0, '等待开始', false);
      updateStats(0, 0);
      showToast('无法开始解密：' + errText(err), 'error');
      if (state.auto && state.auto.fail) state.auto.fail('start_batch 失败：' + errText(err));
    }
  }

  /* ------------------------------------------------------------------ */
  /* UI 绑定                                                             */
  /* ------------------------------------------------------------------ */
  function bindUiEvents() {
    els.btnAdd.addEventListener('click', addFilesFromDialog);
    els.btnClear.addEventListener('click', clearFiles);
    els.dropzone.addEventListener('click', addFilesFromDialog);
    els.dropzone.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); addFilesFromDialog(); }
    });

    els.fileList.addEventListener('click', (e) => {
      const btn = e.target && e.target.closest ? e.target.closest('.row-remove') : null;
      if (!btn) return;
      const rowNode = btn.closest('.file-row');
      if (rowNode && rowNode.dataset.path) removeFile(rowNode.dataset.path);
    });

    els.btnTogglePassword.addEventListener('click', () => {
      setPasswordVisibility(els.passwordInput.type === 'password');
    });
    els.btnClearPassword.addEventListener('click', clearRememberedPassword);
    els.passwordInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') startBatch();
    });

    els.btnChooseOut.addEventListener('click', chooseOutputDir);
    els.btnResetOut.addEventListener('click', resetOutputDir);
    els.conflictSelect.addEventListener('change', updateConflictHint);
    els.btnStart.addEventListener('click', startBatch);
    els.btnOpenOut.addEventListener('click', async () => {
      if (!state.openOutputDir) return;
      if (!invoke) return;
      try {
        await invoke('reveal_path', { path: state.openOutputDir });
      } catch (err) {
        showToast('无法打开输出目录：' + errText(err), 'warn');
      }
    });

    // 避免把拖入的文件当成页面导航
    document.addEventListener('dragover', (e) => e.preventDefault());
    document.addEventListener('drop', (e) => e.preventDefault());
  }

  /* ------------------------------------------------------------------ */
  /* 自动验证（仅在应用以 --autotest-spec/--autotest-report 启动时启用）  */
  /* ------------------------------------------------------------------ */
  function collectRows() {
    return state.files.map((file, i) => {
      const row = state.rowRefs[i];
      return {
        name: file.name,
        path: file.path,
        size: file.size,
        status: row ? row.status.textContent : null,
        statusTitle: row ? row.status.title : null,
        classes: row ? Array.from(row.node.classList) : [],
        fill: row ? row.fill.style.width : null,
        badge: row ? row.badge.textContent : null
      };
    });
  }

  function collectDom() {
    return {
      overallLabel: els.overallLabel ? els.overallLabel.textContent : null,
      overallPct: els.overallPct ? els.overallPct.textContent : null,
      overallFill: els.overallFill ? els.overallFill.style.width : null,
      statOk: els.statOk ? els.statOk.textContent : null,
      statFail: els.statFail ? els.statFail.textContent : null,
      statTotal: els.statTotal ? els.statTotal.textContent : null,
      fileCount: els.fileCount ? els.fileCount.textContent : null,
      startDisabled: els.btnStart ? els.btnStart.disabled : null,
      startLabel: els.btnStart ? (els.btnStart.querySelector('.start-label') || {}).textContent : null,
      passwordFilled: !!(els.passwordInput && els.passwordInput.value),
      passwordLength: els.passwordInput ? els.passwordInput.value.length : 0,
      rememberChecked: !!(els.rememberPassword && els.rememberPassword.checked),
      outputDirShown: els.outputDirDisplay ? (els.outputDirDisplay.textContent || '').trim() : null,
      outputDirCustom: !!(els.outputDirDisplay && els.outputDirDisplay.classList.contains('is-custom')),
      outputHint: els.outputHint ? els.outputHint.textContent : null,
      conflictValue: els.conflictSelect ? els.conflictSelect.value : null,
      conflictHint: els.conflictHint ? els.conflictHint.textContent : null,
      statusHint: els.statusHint ? els.statusHint.textContent : null,
      configHint: els.configHint ? els.configHint.textContent : null,
      toastVisible: !!(els.toast && !els.toast.hidden && els.toast.classList.contains('is-visible')),
      toastText: els.toast ? els.toast.textContent : null,
      emptyHintVisible: !!(els.emptyHint && !els.emptyHint.hidden),
      dragDrop: {
        apiAvailable: state.dragDropApi,
        bound: state.dragDropBound,
        error: state.dragDropError
      }
    };
  }

  async function finishAutotest(report) {
    const auto = state.auto;
    if (!auto) return;
    auto.finished = true;
    report.durationMs = Date.now() - auto.startedAt;
    report.finishedAt = new Date().toISOString();
    try {
      await invoke('autotest_finish', { reportJson: JSON.stringify(report, null, 2) });
    } catch (err) {
      document.title = 'AUTOTEST_FINISH_FAILED: ' + errText(err);
      try { await invoke('autotest_exit'); } catch (exitErr) { /* 退出兜底失败时保持应用运行 */ }
    }
  }

  async function runAutotest(spec, reportPath) {
    const auto = {
      spec, reportPath, startedAt: Date.now(), progressCount: 0, fileFinishedCount: 0,
      lastProgress: null, resolveBatchDone: null, finished: false, timer: 0
    };
    state.auto = auto;
    window.__PARANOIA_AUTOTEST__ = { spec, reportPath };

    const report = {
      name: spec.name || 'unnamed',
      mode: spec.mode || 'batch',
      startedAt: new Date().toISOString(),
      reportPath: reportPath || null,
      probe: {
        tauriInvoke: !!invoke,
        tauriEventListen: !!listen,
        webviewApi: !!getCurrentWebview,
        crateStartupMs: state.initialStartupMs,
        userAgent: navigator.userAgent,
        location: String(window.location.href)
      },
      errors: [],
      rows: [],
      dom: {}
    };

    auto.fail = (msg) => {
      if (auto.finished) return;
      report.errors.push(String(msg));
      report.rows = collectRows();
      report.dom = collectDom();
      finishAutotest(report);
    };

    try {
      if (spec.mode === 'initial') {
        report.initialState = {
          rememberedPasswordPresent: !!state.initialRememberedPassword,
          rememberedPasswordAutoFilled: !!(els.passwordInput && els.passwordInput.value),
          rememberedCheckboxChecked: !!(els.rememberPassword && els.rememberPassword.checked),
          configPath: state.configPath || null,
          passwordValueLength: els.passwordInput ? els.passwordInput.value.length : 0,
          startupMs: state.initialStartupMs
        };
        report.rows = collectRows();
        report.dom = collectDom();
        await finishAutotest(report);
        return;
      }

      if (spec.mode === 'dragdrop-live') {
        // 真实 OS 拖拽：外部脚本在浏览器窗口上执行 Finder 拖拽，这里等待并记录原生事件。
        report.dragDropLive = { events: [], dropped: false, timeoutMs: Number(spec.timeoutMs) || 30000 };
        const deadline = Date.now() + report.dragDropLive.timeoutMs;
        let dropped = false;
        while (Date.now() < deadline) {
          if (state.dragDropEvents.some((e) => e.type === 'drop' && Array.isArray(e.paths) && e.paths.length)) {
            dropped = true;
            break;
          }
          await sleep(250);
        }
        report.dragDropLive.dropped = dropped;
        report.dragDropLive.rawEventCount = state.dragDropEvents.length;
        report.dragDropLive.events = state.dragDropEvents.slice(0, 40);
        await sleep(1500);
        report.rows = collectRows();
        report.dom = collectDom();
        report.addValidations = auto.addValidations || [];
        await finishAutotest(report);
        return;
      }

      if (spec.mode === 'dragdrop') {
        const simulate = window.__PARANOIA_AUTOTEST_DRAG__;
        report.dragDropSim = { available: typeof simulate === 'function', steps: [] };
        if (typeof simulate !== 'function') {
          report.errors.push('拖拽模拟钩子不可用（onDragDropEvent 未绑定成功）');
        } else {
          const record = (step) => {
            report.dragDropSim.steps.push({
              step,
              dropzoneActive: !!(els.dropzone && els.dropzone.classList.contains('is-drag')),
              fileCount: state.files.length,
              countBadge: els.fileCount ? els.fileCount.textContent : null
            });
          };
          simulate({ type: 'enter', paths: [] });
          record('enter');
          simulate({ type: 'over', position: { x: 120, y: 90 } });
          record('over');
          simulate({ type: 'leave' });
          record('leave');
          simulate({ type: 'drop', paths: Array.isArray(spec.files) ? spec.files : [] });
          record('drop-immediate');
          await sleep(1200);
          record('drop-after-1200ms');
          report.addValidations = auto.addValidations || [];
        }
        report.rows = collectRows();
        report.dom = collectDom();
        await finishAutotest(report);
        return;
      }

      if (spec.mode === 'clear') {
        try {
          await invoke('clear_remembered_password');
          report.clearResult = 'ok';
        } catch (err) {
          report.clearResult = 'error: ' + errText(err);
        }
        const initial = await invoke('get_initial_state');
        report.afterClear = {
          rememberedPasswordPresent: !!initial.rememberedPassword,
          configPath: initial.configPath || null
        };
        report.dom = collectDom();
        await finishAutotest(report);
        return;
      }

      // 默认 batch 模式
      if (spec.password != null) els.passwordInput.value = String(spec.password);
      if (spec.remember === true) els.rememberPassword.checked = true;
      if (spec.conflict) {
        els.conflictSelect.value = spec.conflict;
        updateConflictHint();
      }
      if (Array.isArray(spec.files) && spec.files.length) {
        await addPaths(spec.files);
        if (spec.addTwice === true) {
          // 重复添加同一批文件：验证去重分支（duplicates>0）与提示文案
          await sleep(80);
          await addPaths(spec.files);
          report.dedupe = {
            toastText: els.toast ? els.toast.textContent : null,
            toastVisible: !!(els.toast && !els.toast.hidden && els.toast.classList.contains('is-visible')),
            fileCount: state.files.length,
            addValidations: (state.auto.addValidations || []).map((v) => ({
              requested: v.requested,
              accepted: v.accepted,
              duplicates: v.duplicates,
              rejected: (v.rejected || []).length
            }))
          };
        }
      } else {
        report.errors.push('spec.files 为空');
      }
      if (spec.outputDir) {
        state.customOutDir = String(spec.outputDir);
        updateOutDirDisplay();
      }
      await sleep(120);

      if (els.btnStart.disabled) {
        report.errors.push('开始按钮处于禁用状态，无法启动批量解密');
        report.rows = collectRows();
        report.dom = collectDom();
        await finishAutotest(report);
        return;
      }

      const done = new Promise((resolve) => { auto.resolveBatchDone = resolve; });
      els.btnStart.click();

      setTimeout(() => {
        if (!state.running && !state.lastSummary) {
          auto.fail('点击开始后 8 秒内未进入运行状态（start_batch 可能失败）');
        }
      }, 8000);

      auto.timer = setTimeout(() => {
        auto.fail('批量解密超时（240 秒）');
      }, 240000);

      await done;
      clearTimeout(auto.timer);
      await sleep(150);

      report.summary = state.lastSummary;
      report.progressEventCount = auto.progressCount;
      report.fileFinishedEventCount = auto.fileFinishedCount;
      report.lastProgress = auto.lastProgress;
      report.addValidations = auto.addValidations || [];
      report.rows = collectRows();
      report.dom = collectDom();
      await finishAutotest(report);
    } catch (err) {
      auto.fail('autotest 异常：' + errText(err));
    }
  }

  /* ------------------------------------------------------------------ */
  /* 初始化                                                              */
  /* ------------------------------------------------------------------ */
  async function init() {
    cacheEls();
    bindUiEvents();
    setPasswordVisibility(false);
    updateConflictHint();
    updateOutDirDisplay();

    if (!invoke) {
      setStatus('未检测到 Tauri 运行环境', 'error');
      showToast('当前页面不在 Tauri 应用内运行，文件操作不可用', 'error', 8000);
      return;
    }

    await bindTauriEvents();
    await bindDragDrop();

    let initial = null;
    try {
      initial = await invoke('get_initial_state');
    } catch (err) {
      showToast('初始化失败：' + errText(err), 'error');
    }

    if (initial) {
      state.configPath = initial.configPath || null;
      state.initialRememberedPassword = initial.rememberedPassword || null;
      state.initialAutotest = initial.autotest || null;
      state.initialStartupMs = typeof initial.startupMs === 'number' ? initial.startupMs : null;
      if (initial.rememberedPassword) {
        els.passwordInput.value = initial.rememberedPassword;
        els.rememberPassword.checked = true;
      }
      updateConfigHint(initial);
      if (initial.appVersion && els.statusHint) {
        els.statusHint.textContent = '就绪 · v' + initial.appVersion;
      }
    }
    updateOutDirDisplay();

    try {
      await invoke('ui_ready');
    } catch (err) { /* 启动计时日志失败不影响使用 */ }

    const autoInfo = state.initialAutotest;
    if (autoInfo && autoInfo.specJson) {
      const reportPath = autoInfo.reportPath;
      let spec = null;
      try {
        spec = JSON.parse(autoInfo.specJson);
      } catch (err) {
        state.auto = { startedAt: Date.now(), finished: false };
        await finishAutotest({
          name: 'parse-error',
          mode: 'unknown',
          errors: ['spec JSON 解析失败：' + errText(err)],
          rawSpec: autoInfo.specJson
        });
        void reportPath;
        return;
      }
      await runAutotest(spec, reportPath);
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
