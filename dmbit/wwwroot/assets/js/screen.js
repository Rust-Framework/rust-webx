/* global echarts, DmbitApi */
(function () {
  const BLUE = '#2563eb';
  const BLUE_DEEP = '#1d4ed8';
  const PURPLE = '#7c3aed';
  const SKY = '#0ea5e9';
  const INDIGO = '#6366f1';
  const SLATE = '#64748b';
  const INK = '#0f172a';

  /** 与页面蓝紫玻璃风一致的主色板 */
  const PALETTE = [BLUE, PURPLE, SKY, INDIGO, '#93c5fd', '#c4b5fd', '#38bdf8', '#a5b4fc'];

  /** 状态柱：偏玻璃风的柔和色，仍可区分 */
  const STATUS_COLORS = {
    '运行中': '#10b981',
    '联调中': '#f59e0b',
    '待上架': '#94a3b8',
    '已交付': BLUE,
  };

  const fmt = (n, digits) => {
    const x = Number(n || 0);
    const d = digits || 0;
    return x.toLocaleString('zh-CN', {
      minimumFractionDigits: d,
      maximumFractionDigits: d,
    });
  };

  function animateText(el, value, digits) {
    if (!el) return;
    digits = digits || 0;
    const from = Number(el.dataset.val || 0);
    const to = Number(value || 0);
    el.dataset.val = String(to);
    const start = performance.now();
    const tick = (t) => {
      const p = Math.min(1, (t - start) / 650);
      const eased = 1 - Math.pow(1 - p, 3);
      const cur = from + (to - from) * eased;
      el.textContent = fmt(digits ? cur : Math.round(cur), digits);
      if (p < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }

  function tickClock() {
    const el = document.getElementById('datetime');
    if (!el) return;
    const now = new Date();
    const pad = (n) => String(n).padStart(2, '0');
    el.textContent =
      now.getFullYear() +
      '-' +
      pad(now.getMonth() + 1) +
      '-' +
      pad(now.getDate()) +
      '  ' +
      pad(now.getHours()) +
      ':' +
      pad(now.getMinutes()) +
      ':' +
      pad(now.getSeconds());
  }

  function markRefresh() {
    const el = document.getElementById('data-refresh');
    if (!el) return;
    const now = new Date();
    const pad = (n) => String(n).padStart(2, '0');
    el.textContent =
      '刷新 ' + pad(now.getHours()) + ':' + pad(now.getMinutes()) + ':' + pad(now.getSeconds());
  }

  function getChart(id) {
    const el = document.getElementById(id);
    if (!el || !window.echarts) return null;
    return echarts.getInstanceByDom(el) || echarts.init(el);
  }

  function emptyOption(msg) {
    return {
      title: {
        text: msg,
        left: 'center',
        top: 'middle',
        textStyle: { color: SLATE, fontSize: 13, fontWeight: 400 },
      },
    };
  }

  function toSeriesData(list) {
    return (list || [])
      .map((d) => ({ name: d.name, value: Number(d.value) || 0 }))
      .filter((d) => d.value > 0)
      .sort((a, b) => b.value - a.value);
  }

  /**
   * 加速卡 — 参考 pie-custom：玫瑰半径 + 轻阴影，偏蓝紫
   * https://echarts.apache.org/examples/zh/editor.html?c=pie-custom
   */
  function renderAccelChart(list) {
    const chart = getChart('chart-accel');
    if (!chart) return;
    const data = toSeriesData(
      (list || []).map((d) => ({
        name: d.label || d.model || '—',
        value: d.count || 0,
      }))
    );
    if (!data.length) {
      chart.setOption(emptyOption('暂无加速卡数据'), true);
      return;
    }
    chart.setOption(
      {
        color: PALETTE,
        tooltip: {
          trigger: 'item',
          formatter: (p) => p.name + '<br/>' + fmt(p.value) + ' 张（' + p.percent + '%）',
        },
        series: [
          {
            type: 'pie',
            radius: ['18%', '68%'],
            center: ['50%', '52%'],
            roseType: 'radius',
            data,
            itemStyle: {
              borderRadius: 6,
              borderColor: 'rgba(255,255,255,0.92)',
              borderWidth: 2,
              shadowBlur: 18,
              shadowColor: 'rgba(37, 99, 235, 0.18)',
            },
            label: {
              color: '#475569',
              formatter: '{b}\n{c}',
              fontSize: 11,
              lineHeight: 15,
            },
            labelLine: {
              length: 12,
              length2: 10,
              smooth: 0.2,
              lineStyle: { color: 'rgba(100, 116, 139, 0.45)' },
            },
            animationType: 'scale',
            animationEasing: 'elasticOut',
            animationDelay: (idx) => idx * 40,
          },
        ],
      },
      true
    );
  }

  /**
   * 硬盘 — 参考 pie-half-donut：半环 + 中心合计
   * https://echarts.apache.org/examples/zh/editor.html?c=pie-half-donut
   */
  function renderDiskChart(list) {
    const chart = getChart('chart-disk');
    if (!chart) return;
    const data = toSeriesData(
      (list || []).map((d) => ({
        name: d.label || [d.model, d.capacity].filter(Boolean).join(' ') || '—',
        value: d.count || 0,
      }))
    );
    if (!data.length) {
      chart.setOption(emptyOption('暂无硬盘数据'), true);
      return;
    }
    const total = data.reduce((s, d) => s + d.value, 0);
    chart.setOption(
      {
        color: [PURPLE, INDIGO, SKY, BLUE, '#a78bfa', '#818cf8'],
        tooltip: {
          trigger: 'item',
          formatter: (p) => p.name + '<br/>' + fmt(p.value) + ' 块（' + p.percent + '%）',
        },
        legend: {
          bottom: 4,
          left: 'center',
          itemWidth: 10,
          itemHeight: 10,
          textStyle: { color: '#475569', fontSize: 11 },
        },
        series: [
          {
            type: 'pie',
            radius: ['52%', '78%'],
            center: ['50%', '68%'],
            startAngle: 180,
            endAngle: 360,
            data,
            itemStyle: {
              borderRadius: 4,
              borderColor: '#fff',
              borderWidth: 2,
            },
            label: { show: false },
            emphasis: {
              label: {
                show: true,
                fontSize: 12,
                fontWeight: 600,
                color: INK,
                formatter: '{b}\n{c} 块',
              },
              scaleSize: 6,
            },
          },
        ],
        graphic: [
          {
            type: 'text',
            left: 'center',
            top: '52%',
            style: {
              text: fmt(total),
              fill: INK,
              fontSize: 26,
              fontWeight: 650,
              fontFamily: 'JetBrains Mono, ui-monospace, monospace',
              align: 'center',
              verticalAlign: 'middle',
            },
          },
          {
            type: 'text',
            left: 'center',
            top: '62%',
            style: {
              text: '硬盘合计 · 块',
              fill: SLATE,
              fontSize: 12,
              align: 'center',
              verticalAlign: 'middle',
            },
          },
        ],
      },
      true
    );
  }

  /**
   * 产品构成 — 参考 pie-labelLine-adjust：外侧标签 + 引线贴边
   * https://echarts.apache.org/examples/zh/editor.html?c=pie-labelLine-adjust
   */
  function renderTypeChart(devices) {
    const chart = getChart('chart-type');
    if (!chart) return;
    const map = new Map();
    (devices || []).forEach((d) => {
      const k = d.product_name || '其他';
      map.set(k, (map.get(k) || 0) + (d.quantity || 0));
    });
    const data = toSeriesData(
      [...map.entries()].map(([name, value]) => ({ name, value }))
    );
    if (!data.length) {
      chart.setOption(emptyOption('暂无产品数据'), true);
      return;
    }
    chart.setOption(
      {
        color: PALETTE,
        tooltip: {
          trigger: 'item',
          formatter: (p) => p.name + '<br/>' + fmt(p.value) + ' 台（' + p.percent + '%）',
        },
        series: [
          {
            type: 'pie',
            radius: ['34%', '58%'],
            center: ['50%', '50%'],
            data,
            itemStyle: {
              borderColor: '#fff',
              borderWidth: 2,
              borderRadius: 3,
            },
            label: {
              alignTo: 'edge',
              minMargin: 5,
              edgeDistance: 10,
              lineHeight: 16,
              formatter: '{name|{b}}\n{val|{c} 台  {d}%}',
              rich: {
                name: {
                  fontSize: 12,
                  color: '#334155',
                  fontWeight: 600,
                },
                val: {
                  fontSize: 11,
                  color: SLATE,
                },
              },
            },
            labelLine: {
              length: 14,
              length2: 0,
              maxSurfaceAngle: 80,
              lineStyle: { color: 'rgba(100, 116, 139, 0.4)' },
            },
            labelLayout: (params) => {
              const isLeft = params.labelRect.x < chart.getWidth() / 2;
              const points = params.labelLinePoints;
              if (!points || !points[2]) return {};
              points[2][0] = isLeft
                ? params.labelRect.x
                : params.labelRect.x + params.labelRect.width;
              return { labelLinePoints: points };
            },
          },
        ],
      },
      true
    );
  }

  /**
   * 运行状态 — 横向圆角条 + 软轨道（玻璃蓝紫风）
   */
  function renderStatusChart(stats) {
    const chart = getChart('chart-status');
    if (!chart) return;
    const buckets = (stats.status_buckets || []).filter((b) => (b.quantity || 0) > 0);
    if (!buckets.length) {
      chart.setOption(emptyOption('暂无状态数据'), true);
      return;
    }
    const names = buckets.map((b) => b.status || '—');
    const values = buckets.map((b) => b.quantity || 0);
    chart.setOption(
      {
        grid: { left: 78, right: 52, top: 18, bottom: 18 },
        tooltip: {
          trigger: 'axis',
          axisPointer: { type: 'none' },
          backgroundColor: 'rgba(255,255,255,0.92)',
          borderColor: 'rgba(148,163,184,0.25)',
          borderWidth: 1,
          textStyle: { color: INK, fontSize: 12 },
          formatter: (items) => {
            const it = items[0];
            return it.name + '<br/>' + fmt(it.value) + ' 台';
          },
        },
        xAxis: {
          type: 'value',
          minInterval: 1,
          splitLine: {
            lineStyle: { color: 'rgba(148, 163, 184, 0.16)', type: 'dashed', width: 1 },
          },
          axisLine: { show: false },
          axisTick: { show: false },
          axisLabel: { color: 'rgba(100, 116, 139, 0.85)', fontSize: 11 },
        },
        yAxis: {
          type: 'category',
          data: names,
          inverse: true,
          axisTick: { show: false },
          axisLine: { show: false },
          axisLabel: {
            color: '#475569',
            fontSize: 12,
            fontWeight: 500,
            margin: 12,
          },
        },
        series: [
          {
            type: 'bar',
            data: values.map((v, i) => {
              const c = STATUS_COLORS[names[i]] || PURPLE;
              const soft = echarts.color.lift(c, 0.28);
              const deep = echarts.color.lift(c, -0.08);
              return {
                value: v,
                itemStyle: {
                  borderRadius: 999,
                  color: new echarts.graphic.LinearGradient(0, 0, 1, 0, [
                    { offset: 0, color: soft },
                    { offset: 0.55, color: c },
                    { offset: 1, color: deep },
                  ]),
                  shadowBlur: 12,
                  shadowColor: echarts.color.modifyAlpha(c, 0.22),
                  shadowOffsetY: 2,
                },
              };
            }),
            barWidth: 16,
            barCategoryGap: '48%',
            showBackground: true,
            backgroundStyle: {
              color: 'rgba(148, 163, 184, 0.12)',
              borderRadius: 999,
              borderColor: 'rgba(255, 255, 255, 0.55)',
              borderWidth: 1,
            },
            label: {
              show: true,
              position: 'right',
              distance: 8,
              color: '#334155',
              fontSize: 12,
              fontWeight: 600,
              fontFamily: 'JetBrains Mono, ui-monospace, monospace',
              formatter: (p) => fmt(p.value),
            },
            emphasis: {
              itemStyle: {
                shadowBlur: 18,
                shadowColor: 'rgba(37, 99, 235, 0.28)',
              },
            },
            animationDelay: (idx) => idx * 70,
          },
        ],
        animationEasing: 'cubicOut',
      },
      true
    );
  }

  function render(data) {
    const brandEl = document.getElementById('brand-name');
    if (brandEl) {
      const brand = (data.brand_name || '').trim();
      brandEl.textContent = brand;
      brandEl.hidden = !brand;
    }
    document.getElementById('page-title').textContent =
      data.title || '智算机房设备概览';
    document.title = data.title || '智算机房设备概览';
    document.getElementById('room-name').textContent =
      data.room_name || '直播数据智算机房';

    const s = data.stats || {};
    animateText(document.getElementById('kpi-total'), s.total_quantity);
    animateText(document.getElementById('kpi-compute'), s.compute_quantity);
    animateText(document.getElementById('kpi-storage'), s.storage_quantity);
    animateText(document.getElementById('kpi-pb'), s.storage_pb, 1);

    renderAccelChart(data.accelerator_totals);
    renderDiskChart(data.disk_totals);
    renderTypeChart(data.devices || []);
    renderStatusChart(s);
    markRefresh();
  }

  async function load() {
    try {
      const data = await window.DmbitApi.get('/api/dashboard');
      render(data);
    } catch (err) {
      ['chart-accel', 'chart-disk', 'chart-type', 'chart-status'].forEach((id) => {
        const chart = getChart(id);
        if (chart) chart.setOption(emptyOption('加载失败'), true);
      });
    }
  }

  function resizeAll() {
    ['chart-accel', 'chart-disk', 'chart-type', 'chart-status'].forEach((id) => {
      const el = document.getElementById(id);
      const chart = el && window.echarts && echarts.getInstanceByDom(el);
      if (chart) chart.resize();
    });
  }

  window.addEventListener('resize', resizeAll);
  tickClock();
  setInterval(tickClock, 1000);
  load();
  setInterval(load, 60000);
})();
