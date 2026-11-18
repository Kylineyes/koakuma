'use strict';

// koakuma 前端监控脚本：拉取设备列表、实时速率与历史曲线。
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

var deviceSelect = document.getElementById('device-select');
var rangeSelect = document.getElementById('range-select');
var refreshBtn = document.getElementById('refresh-btn');
var applyBtn = document.getElementById('apply-btn');
var resetBtn = document.getElementById('reset-btn');
var fromInput = document.getElementById('from-input');
var toInput = document.getElementById('to-input');
var rxValue = document.getElementById('rx-value');
var txValue = document.getElementById('tx-value');
var chartHint = document.getElementById('chart-hint');
var statusEl = document.getElementById('status');
var canvas = document.getElementById('traffic-chart');

var chart = null;
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

function getSelectedDev() {
  if (!deviceSelect || deviceSelect.selectedIndex < 0) { return null; }
  var opt = deviceSelect.options[deviceSelect.selectedIndex];
  if (!opt) { return null; }
  return opt.value;
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

// 拉取设备列表并填充设备下拉；标记“本机”设备并默认选中它。
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

  deviceSelect.innerHTML = '';
  var selfIndex = -1;
  for (var i = 0; i < list.length; i++) {
    var d = list[i] || {};
    var opt = document.createElement('option');
    opt.value = String(d.id);
    var name = d.name ? d.name : '未命名设备';
    var ip = d.ip ? d.ip : '未知 IP';
    var mac = d.mac ? d.mac : '未知 MAC';
    var isSelf = d.self === true;
    if (isSelf) { selfIndex = i; }
    // 显示格式：[★本机] name（ip）[mac]
    opt.textContent = (isSelf ? '★【本机】 ' : '') + name + '（' + ip + '）[' + mac + ']';
    deviceSelect.appendChild(opt);
  }

  if (list.length === 0) {
    setStatus('暂无设备数据', true);
    rxValue.textContent = '—';
    txValue.textContent = '—';
    setHint('暂无设备，无法绘制曲线。', true);
    return;
  }

  // 默认选中“本机”设备；若无法识别本机则选第一个。
  deviceSelect.selectedIndex = selfIndex >= 0 ? selfIndex : 0;
  if (selfIndex >= 0) {
    setStatus('设备列表已更新（共 ' + list.length + ' 台，已定位到本机）', false);
  } else {
    setStatus('设备列表已更新（共 ' + list.length + ' 台）', false);
  }
}

// 拉取最近一次采样，展示所选设备的下载/上传实时速率。
async function refreshLatest() {
  var dev = getSelectedDev();
  if (dev === null) { return; }
  try {
    var res = await fetch('/api/latest', { headers: { 'Accept': 'application/json' } });
    if (!res.ok) {
      throw new Error('实时速率请求失败：HTTP ' + res.status);
    }
    var data = await res.json();
    var devices = (data && Array.isArray(data.devices)) ? data.devices : [];
    var item = null;
    for (var i = 0; i < devices.length; i++) {
      if (String(devices[i].id) === String(dev)) {
        item = devices[i];
        break;
      }
    }
    if (item) {
      rxValue.textContent = formatRate(item.rx_rate);
      txValue.textContent = formatRate(item.tx_rate);
    } else {
      rxValue.textContent = '—';
      txValue.textContent = '—';
      setStatus('所选设备暂无实时数据', false);
    }
  } catch (err) {
    setStatus(err && err.message ? err.message : '实时速率加载失败', true);
  }
}

// 创建（一次）Chart.js 双折线图实例。
function createChart() {
  if (chart || !canvas || typeof Chart === 'undefined') {
    return chart;
  }
  var ctx = canvas.getContext('2d');
  chart = new Chart(ctx, {
    type: 'line',
    data: {
      datasets: [
        {
          label: '下载 rx',
          data: [],
          borderColor: '#2f6fed',
          backgroundColor: 'rgba(47, 111, 237, 0.12)',
          fill: true,
          tension: 0.25,
          borderWidth: 2,
          pointRadius: 0,
          pointHitRadius: 8
        },
        {
          label: '上传 tx',
          data: [],
          borderColor: '#e8833a',
          backgroundColor: 'rgba(232, 131, 58, 0.12)',
          fill: true,
          tension: 0.25,
          borderWidth: 2,
          pointRadius: 0,
          pointHitRadius: 8
        }
      ]
    },
    options: {
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
            callback: function (value) { return formatTimeTick(value); }
          },
          grid: { color: 'rgba(16, 24, 40, 0.06)' }
        },
        y: {
          beginAtZero: true,
          ticks: {
            maxTicksLimit: 6,
            callback: function (value) { return formatRate(value); }
          },
          grid: { color: 'rgba(16, 24, 40, 0.06)' }
        }
      },
      plugins: {
        legend: { labels: { boxWidth: 12, usePointStyle: true } },
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
    }
  });
  return chart;
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

// 按当前视图拉取历史序列并重绘。
async function refreshSeries() {
  var dev = getSelectedDev();
  if (dev === null) { return; }
  var view = computeView();
  if (view.error) { setHint(view.error, true); return; }
  currentSpan = view.span;
  var url = '/api/series?dev=' + encodeURIComponent(dev) +
    '&layer=' + encodeURIComponent(view.layer) +
    '&from=' + view.from + '&to=' + view.to;

  setHint('正在加载历史数据…', false);
  try {
    var res = await fetch(url, { headers: { 'Accept': 'application/json' } });
    if (!res.ok) {
      throw new Error('历史数据请求失败：HTTP ' + res.status);
    }
    var data = await res.json();
    var points = (data && Array.isArray(data.points)) ? data.points : [];
    var rx = [];
    var tx = [];
    for (var i = 0; i < points.length; i++) {
      var p = points[i];
      if (!p || p.length < 3) { continue; }
      var x = Number(p[0]) * 1000;
      rx.push({ x: x, y: Number(p[1]) });
      tx.push({ x: x, y: Number(p[2]) });
    }

    var c = createChart();
    if (!c) {
      setHint('图表库未加载，无法绘制曲线。', true);
      return;
    }
    c.data.datasets[0].data = rx;
    c.data.datasets[1].data = tx;
    c.update('none');

    var suffix = autoRefreshOn ? '（自动刷新中）' : '（已暂停自动刷新）';
    if (points.length === 0) {
      setHint('该时间段内暂无数据。' + suffix, false);
    } else {
      setHint('共 ' + points.length + ' 个数据点 · ' + view.label + suffix, false);
    }
  } catch (err) {
    setHint(err && err.message ? err.message : '历史数据加载失败', true);
  }
}

// 重新装载所选设备的历史曲线，并按当前模式决定是否启动自动刷新定时器。
// 预设近实时范围 -> 自动刷新；自定义时间段 -> 停止自动刷新。
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

function onDeviceChange() {
  refreshLatest();
  reloadSeriesForSelection();
}

function onRangeChange() {
  // 选择了预设范围即退出自定义模式
  customMode = false;
  reloadSeriesForSelection();
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

// 重置为初始状态：默认“近 1 小时”并恢复自动刷新。
function resetChart() {
  customMode = false;
  if (fromInput) { fromInput.value = ''; }
  if (toInput) { toInput.value = ''; }
  if (rangeSelect) { rangeSelect.selectedIndex = 0; }
  reloadSeriesForSelection();
  setStatus('已重置为初始状态（自动更新）', false);
}

async function init() {
  initRangeSelect();

  // 预填自定义时间段为最近 1 小时，方便直接查看（初始模式仍为预设自动刷新）。
  var now = new Date();
  var hourAgo = new Date(now.getTime() - 3600 * 1000);
  if (fromInput) { fromInput.value = toLocalInputValue(hourAgo); }
  if (toInput) { toInput.value = toLocalInputValue(now); }

  if (refreshBtn) { refreshBtn.addEventListener('click', function () { refreshSeries(); }); }
  if (applyBtn) { applyBtn.addEventListener('click', applyCustomRange); }
  if (resetBtn) { resetBtn.addEventListener('click', resetChart); }
  if (rangeSelect) { rangeSelect.addEventListener('change', onRangeChange); }
  if (deviceSelect) { deviceSelect.addEventListener('change', onDeviceChange); }

  try {
    await loadDevices();
  } catch (err) {
    setStatus(err && err.message ? err.message : '设备列表加载失败', true);
    return;
  }

  if (!deviceSelect || deviceSelect.options.length === 0) {
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
