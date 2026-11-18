'use strict';

// koakuma 前端监控脚本：拉取设备列表、实时速率与历史曲线。
// 所有请求均为同源相对路径，响应为 UTF-8 JSON。

// 时间范围配置：每项包含 layer（采样层）与 span（回溯秒数）。
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
// 回溯跨度不超过该值（秒）时，视为近实时范围，自动刷新历史曲线。
var REALTIME_MAX_SPAN = 86400;

var deviceSelect = document.getElementById('device-select');
var rangeSelect = document.getElementById('range-select');
var refreshBtn = document.getElementById('refresh-btn');
var rxValue = document.getElementById('rx-value');
var txValue = document.getElementById('tx-value');
var chartHint = document.getElementById('chart-hint');
var statusEl = document.getElementById('status');
var canvas = document.getElementById('traffic-chart');

var chart = null;
var latestTimer = null;
var seriesTimer = null;

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

// 根据当前范围格式化 X 轴刻度：跨度较短显示时分，较长显示月日。
function formatTimeTick(ms) {
  var d = new Date(Number(ms));
  if (isNaN(d.getTime())) { return ''; }
  var range = getSelectedRange();
  var span = range ? range.span : 86400;
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

// 拉取设备列表并填充设备下拉，默认选中第一个。
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
  for (var i = 0; i < list.length; i++) {
    var d = list[i] || {};
    var opt = document.createElement('option');
    opt.value = String(d.id);
    var name = d.name ? d.name : '未命名设备';
    var ip = d.ip ? d.ip : '未知 IP';
    var mac = d.mac ? d.mac : '未知 MAC';
    // 显示格式：name（ip）[mac]
    opt.textContent = name + '（' + ip + '）[' + mac + ']';
    deviceSelect.appendChild(opt);
  }

  if (list.length === 0) {
    setStatus('暂无设备数据', true);
    rxValue.textContent = '—';
    txValue.textContent = '—';
    setHint('暂无设备，无法绘制曲线。', true);
    return;
  }

  deviceSelect.selectedIndex = 0;
  setStatus('设备列表已更新（共 ' + list.length + ' 台）', false);
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

// 按当前设备与时间范围拉取历史序列并重绘。
async function refreshSeries() {
  var dev = getSelectedDev();
  if (dev === null) { return; }
  var range = getSelectedRange();
  var to = Math.floor(Date.now() / 1000);
  var from = to - range.span;
  var url = '/api/series?dev=' + encodeURIComponent(dev) +
    '&layer=' + encodeURIComponent(range.layer) +
    '&from=' + from + '&to=' + to;

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

    if (points.length === 0) {
      setHint('该时间范围内暂无数据。', false);
    } else {
      setHint('共 ' + points.length + ' 个数据点（' + range.label + '）。', false);
    }
  } catch (err) {
    setHint(err && err.message ? err.message : '历史数据加载失败', true);
  }
}

// 重新装载所选设备的历史曲线，并按范围重启自动刷新定时器。
function reloadSeriesForSelection() {
  if (seriesTimer !== null) {
    clearInterval(seriesTimer);
    seriesTimer = null;
  }
  refreshSeries();
  var range = getSelectedRange();
  if (range && range.span <= REALTIME_MAX_SPAN) {
    seriesTimer = setInterval(refreshSeries, SERIES_INTERVAL);
  }
}

function onDeviceChange() {
  refreshLatest();
  reloadSeriesForSelection();
}

function onRangeChange() {
  reloadSeriesForSelection();
}

async function init() {
  initRangeSelect();
  if (refreshBtn) {
    refreshBtn.addEventListener('click', function () { refreshSeries(); });
  }
  if (rangeSelect) {
    rangeSelect.addEventListener('change', onRangeChange);
  }
  if (deviceSelect) {
    deviceSelect.addEventListener('change', onDeviceChange);
  }

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
