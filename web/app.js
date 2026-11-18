'use strict';

// koakuma 前端监控脚本：拉取设备列表、实时速率与历史曲线。
// 支持多设备选择：分开查看（每台设备一张图）或叠加查看（同一张图多色多线）。
// 所有请求均为同源相对路径，响应为 UTF-8 JSON。

// 预设时间范围：每项包含 layer（采样层）与 span（回溯秒数）。
// layer：a = 10 秒原始，b = 1 分钟均值，c = 10 分钟均值。
var RANGES = [
  { key: '1h',   label: '近 1 小时（10 秒原始）',  layer: 'a', span: 3600 },
  { key: '6h',   label: '近 6 小时（1 分钟）',     layer: 'b', span: 21600 },
  { key: '1d',   label: '近 1 天（1 分钟）',       layer: 'b', span: 86400 },
  { key: '7d',   label: '近 7 天（1 分钟）',       layer: 'b', span: 604800 },
  { key: '30d',  label: '近 30 天（10 分钟）',     layer: 'c', span: 2592000 },
  { key: '180d', label: '近 180 天（10 分钟）',    layer: 'c', span: 15552000 }
];

// 实时刷新间隔（毫秒）与近实时范围的历史刷新间隔（毫秒）。
var LATEST_INTERVAL = 5000;
var SERIES_INTERVAL = 10000;
// 预设范围回溯跨度不超过该值（秒）时，视为近实时范围，自动刷新历史曲线。
var REALTIME_MAX_SPAN = 86400;

// 设备调色板：按设备在列表中的索引取色，尽量稳定且互相可区分。
var PALETTE = [
  '#2f6fed', '#e8833a', '#2ea36b', '#d64545', '#8b5cf6',
  '#0ea5e9', '#f59e0b', '#10b981', '#ef4444', '#6366f1',
  '#14b8a6', '#e11d48', '#84cc16', '#a855f7', '#f97316',
  '#0891b2', '#c026d3', '#65a30d', '#dc2626', '#2563eb'
];

// 分开查看时每条曲线的固定色：下载=蓝，上传=橙，保持语义一致。
// 曲线颜色跟随主题；分开查看保持 RX/TX 语义色。
var RX_COLOR = '#3559d9';
var TX_COLOR = '#a45d09';

// 临界阻尼弹簧（ζ=1）插值：position/velocity 为当前状态，stiffness 可调。
// 返回原地更新的状态；每个元素同一时间仅运行一个动画。
function springTo(element, target, stiffness) {
  if (!element) { return; }
  if (element._springFrame) { cancelAnimationFrame(element._springFrame); }
  var reduced = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (reduced || !window.requestAnimationFrame) {
    element.style.setProperty('--spring-scale', target);
    return;
  }
  var state = { position: Number(element._springValue) || 1, velocity: 0 };
  var previous = performance.now();
  var k = stiffness || 180;
  function frame(now) {
    var dt = Math.min((now - previous) / 1000, 0.032);
    previous = now;
    // 半隐式欧拉：临界阻尼角频率 ω=√k，加速度=-2ωv-ω²(x-target)。
    var omega = Math.sqrt(k);
    var acceleration = -2 * omega * state.velocity - omega * omega * (state.position - target);
    state.velocity += acceleration * dt;
    state.position += state.velocity * dt;
    element.style.setProperty('--spring-scale', state.position.toFixed(4));
    element._springValue = state.position;
    if (Math.abs(state.position - target) > 0.001 || Math.abs(state.velocity) > 0.001) {
      element._springFrame = requestAnimationFrame(frame);
    } else {
      element.style.setProperty('--spring-scale', target);
      element._springValue = target;
      element._springFrame = null;
    }
  }
  element._springFrame = requestAnimationFrame(frame);
}
function bindPressSpring(element) {
  if (!element) { return; }
  element.classList.add('spring-press');
  element.addEventListener('pointerdown', function () { springTo(element, 0.965, 220); });
  element.addEventListener('pointerup', function () { springTo(element, 1, 180); });
  element.addEventListener('pointercancel', function () { springTo(element, 1, 180); });
  element.addEventListener('pointerleave', function () { springTo(element, 1, 180); });
  element.addEventListener('blur', function () { springTo(element, 1, 180); });
}

function themeToken(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
function syncThemeColors() {
  RX_COLOR = themeToken('--rx') || '#3559d9';
  TX_COLOR = themeToken('--tx') || '#a45d09';
}
function applyTheme(theme, persist) {
  var dark = theme === 'dark';
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
  var action = dark ? '切换到浅色模式' : '切换到深色模式';
  if (themeToggle) {
    themeToggle.setAttribute('aria-label', action);
    themeToggle.setAttribute('title', action);
    themeToggle.setAttribute('aria-pressed', dark ? 'true' : 'false');
  }
  if (themeLabel) { themeLabel.textContent = dark ? '浅色' : '深色'; }
  var meta = document.querySelector('meta[name="theme-color"]');
  if (meta) { meta.content = dark ? '#10131b' : '#f3f5fa'; }
  if (persist) {
    try { localStorage.setItem('koakuma-theme', dark ? 'dark' : 'light'); } catch (e) { /* 隐私模式下不持久化 */ }
  }
  syncThemeColors();
  // Chart.js 画布不随 CSS 自动变色，重建后沿用当前视图和数据。
  if (charts.length) {
    chartsKey = '';
    refreshSeries();
  }
}
function initTheme() {
  applyTheme(document.documentElement.dataset.theme || 'light', false);
  bindPressSpring(themeToggle);
  if (themeToggle) {
    themeToggle.addEventListener('click', function () {
      document.documentElement.classList.add('theme-enter');
      applyTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark', true);
      setTimeout(function () { document.documentElement.classList.remove('theme-enter'); }, 500);
    });
  }
  // 未手动选择时跟随系统；手动选择后不再覆盖。
  if (window.matchMedia) {
    var query = matchMedia('(prefers-color-scheme: dark)');
    var listener = function (event) {
      var saved = null;
      try { saved = localStorage.getItem('koakuma-theme'); } catch (e) { /* 忽略 */ }
      if (!saved) { applyTheme(event.matches ? 'dark' : 'light', false); }
    };
    if (query.addEventListener) { query.addEventListener('change', listener); }
    else if (query.addListener) { query.addListener(listener); }
  }
}

function colorForIndex(i) {
  var n = PALETTE.length;
  return PALETTE[((i % n) + n) % n];
}

var deviceChipsEl = document.getElementById('device-chips');
var selectedCountEl = document.getElementById('selected-count');
var selectAllBtn = document.getElementById('select-all-btn');
var clearAllBtn = document.getElementById('clear-all-btn');
var rangeSelect = document.getElementById('range-select');
var modeSelect = document.getElementById('mode-select');
var metricSelect = document.getElementById('metric-select');
var metricField = document.getElementById('metric-field');
var refreshBtn = document.getElementById('refresh-btn');
var applyBtn = document.getElementById('apply-btn');
var resetBtn = document.getElementById('reset-btn');
var fromInput = document.getElementById('from-input');
var toInput = document.getElementById('to-input');
var rxValue = document.getElementById('rx-value');
var txValue = document.getElementById('tx-value');
var rxTitle = document.getElementById('rx-title');
var txTitle = document.getElementById('tx-title');
var chartHint = document.getElementById('chart-hint');
var statusEl = document.getElementById('status');
var chartsContainer = document.getElementById('charts-container');
var themeToggle = document.getElementById('theme-toggle');
var themeLabel = document.getElementById('theme-label');

// 设备列表与选中状态。
var devices = [];
// 已选设备 id（字符串），按设备列表顺序保存。
var selectedIds = [];
// 显示方式：separate（分开查看）/ overlay（叠加查看）。
var mode = 'separate';
// 叠加查看时的指标：rx（下载）/ tx（上传）/ both（下载+上传）。
var overlayMetric = 'both';

// 当前图表实例（分开查看时每台设备一个；叠加查看时仅一个）。
var charts = [];
// 当前图表骨架签名，用于判断是否需要重建 DOM 与图表实例。
var chartsKey = '';

var latestTimer = null;
var seriesTimer = null;
// 是否处于“自定义时间段”模式；此模式下历史曲线停止自动刷新。
var customMode = false;
// 当前曲线实际使用的回溯跨度（秒），用于 X 轴刻度格式选择。
var currentSpan = 86400;
// 当前历史曲线是否在自动刷新。
var autoRefreshOn = false;

// 将字节/秒格式化为人类可读单位：B/s、KB/s、MB/s、GB/s。
function formatRate(bytesPerSec) {
  var n = Number(bytesPerSec);
  if (!isFinite(n) || n < 0) { return '—'; }
  var units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  var idx = 0;
  while (n >= 1024 && idx < units.length - 1) {
    n = n / 1024;
    idx = idx + 1;
  }
  var text;
  if (idx === 0) {
    text = String(Math.round(n));
  } else {
    text = n.toFixed(1);
  }
  return text + ' ' + units[idx];
}

function pad2(v) {
  return v < 10 ? '0' + v : String(v);
}

// 将毫秒时间戳格式化为完整日期时间，供提示框使用。
function formatDateTime(ms) {
  var d = new Date(Number(ms));
  if (isNaN(d.getTime())) { return ''; }
  return d.getFullYear() + '-' + pad2(d.getMonth() + 1) + '-' + pad2(d.getDate()) +
    ' ' + pad2(d.getHours()) + ':' + pad2(d.getMinutes()) + ':' + pad2(d.getSeconds());
}

// 将 Date 格式化为 datetime-local 输入所需的值：YYYY-MM-DDTHH:mm。
function toLocalInputValue(date) {
  return date.getFullYear() + '-' + pad2(date.getMonth() + 1) + '-' + pad2(date.getDate()) +
    'T' + pad2(date.getHours()) + ':' + pad2(date.getMinutes());
}

// 解析 datetime-local 输入为 Unix 秒；非法返回 null。
function parseInputSecs(el) {
  if (!el || !el.value) { return null; }
  var t = new Date(el.value).getTime();
  if (isNaN(t)) { return null; }
  return Math.floor(t / 1000);
}

// 依据跨度选择采样层：<= 6 小时用 10 秒原始，<= 7 天用 1 分钟，否则用 10 分钟。
function pickLayerBySpan(spanSecs) {
  if (spanSecs <= 6 * 3600) { return 'a'; }
  if (spanSecs <= 7 * 86400) { return 'b'; }
  return 'c';
}

function layerName(layer) {
  if (layer === 'a') { return '10 秒原始'; }
  if (layer === 'b') { return '1 分钟'; }
  return '10 分钟';
}

// 根据当前跨度格式化 X 轴刻度：跨度较短显示时分，较长显示月日。
function formatTimeTick(ms) {
  var d = new Date(Number(ms));
  if (isNaN(d.getTime())) { return ''; }
  var span = currentSpan || 86400;
  if (span <= 172800) {
    return pad2(d.getHours()) + ':' + pad2(d.getMinutes());
  }
  return pad2(d.getMonth() + 1) + '-' + pad2(d.getDate());
}

function setStatus(text, isError) {
  if (!statusEl) { return; }
  statusEl.textContent = text || '';
  if (isError) {
    statusEl.classList.add('error');
  } else {
    statusEl.classList.remove('error');
  }
}

function setHint(text, isError) {
  if (!chartHint) { return; }
  chartHint.textContent = text || '';
  if (isError) {
    chartHint.classList.add('error');
  } else {
    chartHint.classList.remove('error');
  }
}

// 按设备 id 查找其在列表中的索引；找不到返回 -1。
function deviceIndexById(id) {
  var key = String(id);
  for (var i = 0; i < devices.length; i++) {
    if (String(devices[i].id) === key) { return i; }
  }
  return -1;
}

function deviceById(id) {
  var idx = deviceIndexById(id);
  return idx >= 0 ? devices[idx] : null;
}

// 设备在列表中的稳定索引色。
function deviceColor(id) {
  return colorForIndex(deviceIndexById(id));
}

function deviceName(d) {
  return (d && d.name) ? d.name : '未命名';
}

// 返回当前已选设备 id 列表，按设备列表顺序过滤掉已不存在的设备。
function getSelectedIds() {
  var out = [];
  for (var i = 0; i < devices.length; i++) {
    var id = String(devices[i].id);
    if (selectedIds.indexOf(id) >= 0) { out.push(id); }
  }
  return out;
}

function sortSelected() {
  selectedIds.sort(function (a, b) {
    return deviceIndexById(a) - deviceIndexById(b);
  });
}

function updateSelectedCount() {
  var n = getSelectedIds().length;
  if (selectedCountEl) { selectedCountEl.textContent = '已选 ' + n + ' 台'; }
}

// 渲染设备芯片/复选项：可点击切换，支持多选。
function renderChips() {
  if (!deviceChipsEl) { return; }
  deviceChipsEl.innerHTML = '';
  for (var i = 0; i < devices.length; i++) {
    var d = devices[i] || {};
    var id = String(d.id);
    var chip = document.createElement('button');
    chip.type = 'button';
    chip.className = 'device-chip' + (selectedIds.indexOf(id) >= 0 ? ' active' : '');
    chip.setAttribute('data-dev-id', id);
    chip.setAttribute('aria-pressed', selectedIds.indexOf(id) >= 0 ? 'true' : 'false');
    chip.setAttribute('aria-label', (d.self === true ? '本机，' : '') + deviceName(d) + '，' + (d.ip || '未知 IP'));
    bindPressSpring(chip);

    var dot = document.createElement('span');
    dot.className = 'chip-dot';
    dot.style.background = colorForIndex(i);
    chip.appendChild(dot);

    var label = document.createElement('span');
    label.className = 'chip-label';
    var name = d.name ? d.name : '未命名';
    var ip = d.ip ? d.ip : '未知 IP';
    var mac = d.mac ? d.mac : '未知 MAC';
    // 显示格式：★本机 name（ip）[mac]
    label.textContent = (d.self === true ? '★本机 ' : '') + name + '（' + ip + '）[' + mac + ']';
    chip.appendChild(label);

    var check = document.createElement('span');
    check.className = 'chip-check';
    check.setAttribute('aria-hidden', 'true');
    check.textContent = '✓';
    chip.appendChild(check);

    (function (devId) {
      chip.addEventListener('click', function () { toggleSelect(devId); });
    })(id);

    deviceChipsEl.appendChild(chip);
  }
  updateSelectedCount();
}

function toggleSelect(id) {
  var idx = selectedIds.indexOf(id);
  if (idx >= 0) {
    selectedIds.splice(idx, 1);
  } else {
    selectedIds.push(id);
  }
  sortSelected();
  renderChips();
  onSelectionChanged();
}

function selectAllDevices() {
  selectedIds = devices.map(function (d) { return String(d.id); });
  sortSelected();
  renderChips();
  onSelectionChanged();
}

function clearAllDevices() {
  selectedIds = [];
  renderChips();
  onSelectionChanged();
}

// 默认选择：存在本机则选本机，否则选第一台。
function resetSelectionToDefault() {
  selectedIds = [];
  if (!devices.length) { return; }
  var selfIdx = -1;
  for (var i = 0; i < devices.length; i++) {
    if (devices[i].self === true) { selfIdx = i; break; }
  }
  selectedIds = [String(devices[selfIdx >= 0 ? selfIdx : 0].id)];
}

function getSelectedRange() {
  if (!rangeSelect || rangeSelect.selectedIndex < 0) { return RANGES[0]; }
  return RANGES[rangeSelect.selectedIndex] || RANGES[0];
}

// 填充时间范围下拉，默认选中“近 1 小时”。
function initRangeSelect() {
  if (!rangeSelect) { return; }
  rangeSelect.innerHTML = '';
  for (var i = 0; i < RANGES.length; i++) {
    var opt = document.createElement('option');
    opt.value = RANGES[i].key;
    opt.textContent = RANGES[i].label;
    rangeSelect.appendChild(opt);
  }
  rangeSelect.selectedIndex = 0;
}

// 拉取设备列表并渲染芯片；默认选中本机（无本机则第一台）。
async function loadDevices() {
  setStatus('正在加载设备列表…', false);
  var res = await fetch('/api/devices', { headers: { 'Accept': 'application/json' } });
  if (!res.ok) {
    throw new Error('设备列表请求失败：HTTP ' + res.status);
  }
  var list = await res.json();
  if (!Array.isArray(list)) {
    throw new Error('设备列表响应格式异常');
  }

  devices = list;
  resetSelectionToDefault();
  renderChips();

  if (list.length === 0) {
    setStatus('暂无设备数据', true);
    if (rxValue) { rxValue.textContent = '—'; }
    if (txValue) { txValue.textContent = '—'; }
    setHint('暂无设备，无法绘制曲线。', true);
    return;
  }

  var hasSelf = false;
  for (var i = 0; i < list.length; i++) {
    if (list[i] && list[i].self === true) { hasSelf = true; break; }
  }
  if (hasSelf) {
    setStatus('设备列表已更新（共 ' + list.length + ' 台，已定位到本机）', false);
  } else {
    setStatus('设备列表已更新（共 ' + list.length + ' 台）', false);
  }
}

// 更新实时卡片标题：单一选中显示设备级标题，多选显示“所选 N 台合计”。
function updateMetricTitles(count) {
  if (rxTitle) {
    rxTitle.textContent = count > 1 ? ('下载速率（所选 ' + count + ' 台合计）') : '下载速率';
  }
  if (txTitle) {
    txTitle.textContent = count > 1 ? ('上传速率（所选 ' + count + ' 台合计）') : '上传速率';
  }
}

// 拉取最近一次采样：单选取该设备 rx/tx，多选取所选设备 rx/tx 之和。
async function refreshLatest() {
  var ids = getSelectedIds();
  if (ids.length === 0) {
    if (rxValue) { rxValue.textContent = '—'; }
    if (txValue) { txValue.textContent = '—'; }
    updateMetricTitles(0);
    return;
  }
  try {
    var res = await fetch('/api/latest', { headers: { 'Accept': 'application/json' } });
    if (!res.ok) {
      throw new Error('实时速率请求失败：HTTP ' + res.status);
    }
    var data = await res.json();
    var list = (data && Array.isArray(data.devices)) ? data.devices : [];
    var map = {};
    for (var i = 0; i < list.length; i++) {
      if (list[i] && list[i].id !== undefined && list[i].id !== null) {
        map[String(list[i].id)] = list[i];
      }
    }

    var found = 0;
    var sumRx = 0;
    var sumTx = 0;
    for (var j = 0; j < ids.length; j++) {
      var item = map[ids[j]];
      if (item) {
        found += 1;
        sumRx += Number(item.rx_rate) || 0;
        sumTx += Number(item.tx_rate) || 0;
      }
    }

    updateMetricTitles(ids.length);
    if (found === 0) {
      if (rxValue) { rxValue.textContent = '—'; }
      if (txValue) { txValue.textContent = '—'; }
      setStatus('所选设备暂无实时数据', false);
      return;
    }
    if (rxValue) { rxValue.textContent = formatRate(sumRx); }
    if (txValue) { txValue.textContent = formatRate(sumTx); }
  } catch (err) {
    setStatus(err && err.message ? err.message : '实时速率加载失败', true);
  }
}

// 计算当前要查询的视图参数（预设范围或自定义时间段）。
function computeView() {
  if (customMode) {
    var f = parseInputSecs(fromInput);
    var t = parseInputSecs(toInput);
    if (f === null || t === null) { return { error: '请选择完整的开始与结束时间' }; }
    if (t <= f) { return { error: '结束时间必须晚于开始时间' }; }
    var span = t - f;
    var layer = pickLayerBySpan(span);
    return {
      layer: layer,
      from: f,
      to: t,
      span: span,
      label: formatDateTime(f * 1000) + ' ~ ' + formatDateTime(t * 1000) + '（' + layerName(layer) + '）'
    };
  }
  var range = getSelectedRange();
  var to2 = Math.floor(Date.now() / 1000);
  return { layer: range.layer, from: to2 - range.span, to: to2, span: range.span, label: range.label };
}

// 通用的图表 options（X 轴 type:linear，用 ts*1000 作为 x，ticks 自绘时间）。
function baseOptions() {
  return {
    responsive: true,
    maintainAspectRatio: false,
    animation: false,
    interaction: { mode: 'index', intersect: false },
    scales: {
      x: {
        type: 'linear',
        ticks: {
          maxRotation: 0,
          autoSkip: true,
          maxTicksLimit: 8,
          color: themeToken('--muted'),
          callback: function (value) { return formatTimeTick(value); }
        },
        grid: { color: themeToken('--chart-grid') }
      },
      y: {
        beginAtZero: true,
        ticks: {
          maxTicksLimit: 6,
          color: themeToken('--muted'),
          callback: function (value) { return formatRate(value); }
        },
        grid: { color: themeToken('--chart-grid') }
      }
    },
    plugins: {
      legend: { labels: { boxWidth: 12, usePointStyle: true, color: themeToken('--text-secondary') } },
      tooltip: {
        callbacks: {
          title: function (items) {
            if (!items.length) { return ''; }
            return formatDateTime(items[0].parsed.x);
          },
          label: function (item) {
            return item.dataset.label + '：' + formatRate(item.parsed.y);
          }
        }
      }
    }
  };
}

// 构造一条折线数据集。
function makeDataset(label, color, dashed) {
  return {
    label: label,
    data: [],
    borderColor: color,
    backgroundColor: color,
    fill: false,
    tension: 0.25,
    borderWidth: 2,
    pointRadius: 0,
    pointHitRadius: 8,
    borderDash: dashed ? [6, 4] : []
  };
}

// 将接口 points（[[ts,rx,tx], ...]）拆成 rx / tx 两个 {x,y} 数组。
function buildPointArrays(points) {
  var rx = [];
  var tx = [];
  if (!Array.isArray(points)) { return { rx: rx, tx: tx }; }
  for (var i = 0; i < points.length; i++) {
    var p = points[i];
    if (!p || p.length < 3) { continue; }
    var x = Number(p[0]) * 1000;
    rx.push({ x: x, y: Number(p[1]) });
    tx.push({ x: x, y: Number(p[2]) });
  }
  return { rx: rx, tx: tx };
}

// 销毁所有图表实例并清空容器。
function destroyCharts() {
  for (var i = 0; i < charts.length; i++) {
    try { charts[i].destroy(); } catch (e) { /* 忽略销毁异常 */ }
  }
  charts = [];
  if (chartsContainer) { chartsContainer.innerHTML = ''; }
}

// 在当前容器中创建一张带标题的图表卡片，返回新建的 Chart 实例。
function createChartCard(titleText, datasets, color) {
  var card = document.createElement('div');
  card.className = 'chart-card';

  var head = document.createElement('div');
  head.className = 'chart-card-head';
  var dot = document.createElement('span');
  dot.className = 'chip-dot';
  if (color) { dot.style.background = color; } else { dot.style.display = 'none'; }
  var title = document.createElement('span');
  title.className = 'chart-card-title';
  title.textContent = titleText;
  head.appendChild(dot);
  head.appendChild(title);
  card.appendChild(head);

  var box = document.createElement('div');
  box.className = 'chart-box';
  var canvasEl = document.createElement('canvas');
  box.appendChild(canvasEl);
  card.appendChild(box);

  chartsContainer.appendChild(card);
  if (!window.matchMedia || !matchMedia('(prefers-reduced-motion: reduce)').matches) {
    card.classList.add('chart-enter');
  }

  var inst = new Chart(canvasEl.getContext('2d'), {
    type: 'line',
    data: { datasets: datasets },
    options: baseOptions()
  });
  return inst;
}

// 依据当前 mode / 选中设备构建图表骨架（仅当签名发生变化时才重建）。
function chartsSignature(ids) {
  return mode + '|' + ids.join(',') + '|' + (mode === 'overlay' ? overlayMetric : '-');
}

function ensureCharts(ids) {
  var sig = chartsSignature(ids);
  if (sig === chartsKey && charts.length > 0) { return; }
  destroyCharts();
  if (!chartsContainer || typeof Chart === 'undefined') { chartsKey = sig; return; }

  if (mode === 'separate') {
    for (var i = 0; i < ids.length; i++) {
      var d = deviceById(ids[i]) || {};
      var color = deviceColor(ids[i]);
      var rxDs = makeDataset('下载 rx', RX_COLOR, false);
      var txDs = makeDataset('上传 tx', TX_COLOR, false);
      var inst = createChartCard(deviceName(d), [rxDs, txDs], color);
      charts.push(inst);
    }
  } else {
    // 叠加查看：所有设备画在同一张图，标题标注设备数量。
    var overlayTitle = '叠加查看（' + ids.length + ' 台设备）';
    var inst2 = createChartCard(overlayTitle, [], null);
    charts.push(inst2);
  }
  chartsKey = sig;
}

// 分开查看：把每台设备的数据写入对应图表。
function updateSeparateData(results) {
  for (var i = 0; i < charts.length && i < results.length; i++) {
    var arr = buildPointArrays(results[i]);
    charts[i].data.datasets[0].borderColor = RX_COLOR;
    charts[i].data.datasets[0].backgroundColor = RX_COLOR;
    charts[i].data.datasets[1].borderColor = TX_COLOR;
    charts[i].data.datasets[1].backgroundColor = TX_COLOR;
    charts[i].data.datasets[0].data = arr.rx;
    charts[i].data.datasets[1].data = arr.tx;
    charts[i].update('none');
  }
}

// 叠加查看：按所选指标重建数据集。
function updateOverlayData(ids, results) {
  if (!charts.length) { return; }
  var datasets = [];
  for (var i = 0; i < ids.length; i++) {
    var d = deviceById(ids[i]) || {};
    var color = deviceColor(ids[i]);
    var name = deviceName(d);
    var arr = buildPointArrays(results[i]);
    if (overlayMetric === 'rx') {
      var dsRx = makeDataset(name, color, false);
      dsRx.data = arr.rx;
      datasets.push(dsRx);
    } else if (overlayMetric === 'tx') {
      var dsTx = makeDataset(name, color, false);
      dsTx.data = arr.tx;
      datasets.push(dsTx);
    } else {
      // 下载+上传：同设备同色，rx 实线、tx 虚线，图例区分指标。
      var dsBothRx = makeDataset(name + ' · 下载', color, false);
      dsBothRx.data = arr.rx;
      datasets.push(dsBothRx);
      var dsBothTx = makeDataset(name + ' · 上传', color, true);
      dsBothTx.data = arr.tx;
      datasets.push(dsBothTx);
    }
  }
  charts[0].data.datasets = datasets;
  charts[0].update('none');
}

// 按当前视图并行拉取所有已选设备的历史序列并重绘。
async function refreshSeries() {
  var ids = getSelectedIds();
  var view = computeView();
  if (view.error) {
    if (ids.length) { ensureCharts(ids); }
    setHint(view.error, true);
    return;
  }
  currentSpan = view.span;

  if (ids.length === 0) {
    destroyCharts();
    chartsKey = '';
    setHint('请选择至少一台设备以查看历史曲线', false);
    return;
  }
  if (typeof Chart === 'undefined') {
    setHint('图表库未加载，无法绘制曲线。', true);
    return;
  }

  ensureCharts(ids);
  setHint('正在加载历史数据…', false);
  try {
    var results = await Promise.all(ids.map(function (id) {
      return fetchSeries(id, view.layer, view.from, view.to);
    }));

    if (mode === 'separate') {
      updateSeparateData(results);
    } else {
      updateOverlayData(ids, results);
    }

    var total = 0;
    for (var i = 0; i < results.length; i++) { total += results[i].length; }
    var suffix = autoRefreshOn ? '（自动刷新中）' : '（已暂停自动刷新）';
    if (total === 0) {
      setHint('该时间段内暂无数据。' + suffix, false);
    } else {
      setHint('共 ' + ids.length + ' 台设备 · ' + total + ' 个数据点 · ' + view.label + suffix, false);
    }
  } catch (err) {
    setHint(err && err.message ? err.message : '历史数据加载失败', true);
  }
}

// 单设备历史序列请求，返回 points 数组。
async function fetchSeries(devId, layer, from, to) {
  var url = '/api/series?dev=' + encodeURIComponent(devId) +
    '&layer=' + encodeURIComponent(layer) +
    '&from=' + from + '&to=' + to;
  var res = await fetch(url, { headers: { 'Accept': 'application/json' } });
  if (!res.ok) {
    throw new Error('历史数据请求失败：HTTP ' + res.status);
  }
  var data = await res.json();
  return (data && Array.isArray(data.points)) ? data.points : [];
}

// 重新装载历史曲线，并按当前模式决定是否启动自动刷新定时器。
// 预设近实时范围且非自定义 -> 自动刷新；自定义时间段 -> 停止自动刷新。
function reloadSeriesForSelection() {
  if (seriesTimer !== null) {
    clearInterval(seriesTimer);
    seriesTimer = null;
  }
  var view = computeView();
  autoRefreshOn = (!view.error && !customMode && view.span <= REALTIME_MAX_SPAN);
  refreshSeries();
  if (autoRefreshOn) {
    seriesTimer = setInterval(refreshSeries, SERIES_INTERVAL);
  }
}

// 选中设备变化：刷新实时卡片与历史曲线。
function onSelectionChanged() {
  refreshLatest();
  reloadSeriesForSelection();
}

function onRangeChange() {
  // 选择了预设范围即退出自定义模式。
  customMode = false;
  reloadSeriesForSelection();
}

function onModeChange() {
  mode = modeSelect ? modeSelect.value : 'separate';
  if (metricField) { metricField.hidden = (mode !== 'overlay'); }
  reloadSeriesForSelection();
}

function onMetricChange() {
  overlayMetric = metricSelect ? metricSelect.value : 'both';
  if (mode === 'overlay') {
    reloadSeriesForSelection();
  }
}

// 应用自定义时间段：此模式下历史曲线停止自动刷新。
function applyCustomRange() {
  var f = parseInputSecs(fromInput);
  var t = parseInputSecs(toInput);
  if (f === null || t === null) { setHint('请选择完整的开始与结束时间', true); return; }
  if (t <= f) { setHint('结束时间必须晚于开始时间', true); return; }
  customMode = true;
  reloadSeriesForSelection();
  setStatus('已切换到自定义时间段（历史曲线已暂停自动刷新）', false);
}

// 重置为初始状态：近 1 小时、自动刷新、默认显示方式（分开查看 / 下载+上传）、默认选中。
function resetChart() {
  customMode = false;
  mode = 'separate';
  overlayMetric = 'both';
  if (modeSelect) { modeSelect.value = 'separate'; }
  if (metricSelect) { metricSelect.value = 'both'; }
  if (metricField) { metricField.hidden = true; }
  if (rangeSelect) { rangeSelect.selectedIndex = 0; }

  // 恢复自定义输入为最近 1 小时（初始预填值），但不进入自定义模式。
  var now = new Date();
  var hourAgo = new Date(now.getTime() - 3600 * 1000);
  if (fromInput) { fromInput.value = toLocalInputValue(hourAgo); }
  if (toInput) { toInput.value = toLocalInputValue(now); }

  resetSelectionToDefault();
  renderChips();
  onSelectionChanged();
  setStatus('已重置为初始状态（自动更新）', false);
}

// “刷新”按钮：同时刷新实时卡片与历史曲线。
function manualRefresh() {
  refreshLatest();
  refreshSeries();
}

async function init() {
  initTheme();
  initRangeSelect();
  if (modeSelect) { modeSelect.value = 'separate'; }
  if (metricSelect) { metricSelect.value = 'both'; }
  if (metricField) { metricField.hidden = true; }
  mode = 'separate';
  overlayMetric = 'both';

  // 预填自定义时间段为最近 1 小时，方便直接查看（初始模式仍为预设自动刷新）。
  var now = new Date();
  var hourAgo = new Date(now.getTime() - 3600 * 1000);
  if (fromInput) { fromInput.value = toLocalInputValue(hourAgo); }
  if (toInput) { toInput.value = toLocalInputValue(now); }

  if (refreshBtn) { refreshBtn.addEventListener('click', manualRefresh); }
  if (applyBtn) { applyBtn.addEventListener('click', applyCustomRange); }
  if (resetBtn) { resetBtn.addEventListener('click', resetChart); }
  if (rangeSelect) { rangeSelect.addEventListener('change', onRangeChange); }
  if (modeSelect) { modeSelect.addEventListener('change', onModeChange); }
  if (metricSelect) { metricSelect.addEventListener('change', onMetricChange); }
  [selectAllBtn, clearAllBtn, refreshBtn, applyBtn, resetBtn].forEach(bindPressSpring);
  if (selectAllBtn) { selectAllBtn.addEventListener('click', selectAllDevices); }
  if (clearAllBtn) { clearAllBtn.addEventListener('click', clearAllDevices); }

  try {
    await loadDevices();
  } catch (err) {
    setStatus(err && err.message ? err.message : '设备列表加载失败', true);
    return;
  }

  if (!devices.length) {
    return;
  }

  refreshLatest();
  reloadSeriesForSelection();
  latestTimer = setInterval(refreshLatest, LATEST_INTERVAL);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}
