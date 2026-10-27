import React, { useEffect, useMemo, useRef, useState } from 'react';
import { boll, ema, macd, sma, type Series } from './indicators';

/**
 * 自绘 Canvas K 线（主图 + 成交量 + MACD）。
 *
 * 为什么不复用 market 工作台的 Vela：数据工坊是普通插件页，再引入一份图表运行时
 * 会让这个包的依赖体积翻倍；而这里需要的只是「蜡烛 + 几条均线 + 量 + MACD」，
 * Canvas 直绘足够，且指标的取色、十字光标、tooltip 可以完全按产品口径控制。
 * 颜色走宿主 CSS 变量，深色与浅色主题都自动适配。
 */

export interface ChartBar {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

export type OverlayId = 'ma5' | 'ma10' | 'ma20' | 'ma60' | 'ema12' | 'ema26' | 'boll';

export const OVERLAY_META: { id: OverlayId; label: string; color: string }[] = [
  { id: 'ma5', label: 'MA5', color: '#e8a33d' },
  { id: 'ma10', label: 'MA10', color: '#3a6df0' },
  { id: 'ma20', label: 'MA20', color: '#b56cf0' },
  { id: 'ma60', label: 'MA60', color: '#2ba471' },
  { id: 'ema12', label: 'EMA12', color: '#12a4b8' },
  { id: 'ema26', label: 'EMA26', color: '#d54941' },
  { id: 'boll', label: 'BOLL(20,2)', color: '#7a8699' },
];

/** 中国市场惯例：涨红跌绿。 */
const UP = '#d54941';
const DOWN = '#2ba471';

function cssVar(name: string, fallback: string): string {
  try {
    const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return value || fallback;
  } catch {
    return fallback;
  }
}

function fmtPrice(v: number): string {
  if (!Number.isFinite(v)) return '—';
  if (Math.abs(v) >= 1000) return v.toFixed(2);
  if (Math.abs(v) >= 1) return v.toFixed(4);
  return v.toFixed(6);
}

function fmtVolume(v: number): string {
  if (!Number.isFinite(v)) return '—';
  const abs = Math.abs(v);
  if (abs >= 1e9) return `${(v / 1e9).toFixed(2)}B`;
  if (abs >= 1e6) return `${(v / 1e6).toFixed(2)}M`;
  if (abs >= 1e3) return `${(v / 1e3).toFixed(2)}K`;
  return v.toFixed(2);
}

function fmtTime(ts: number, interval: string): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, '0');
  const day = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  if (interval === '1d') return day;
  return `${day} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function KlineChart({
  bars,
  interval,
  overlays,
  showVolume,
  showMacd,
}: {
  bars: ChartBar[];
  interval: string;
  overlays: Set<OverlayId>;
  showVolume: boolean;
  showMacd: boolean;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hover, setHover] = useState<number | null>(null);

  const closes = useMemo(() => bars.map((b) => b.close), [bars]);

  // 指标只在被勾选时计算 —— 1200 根 × 7 条线，全算一遍是纯浪费。
  const series = useMemo(() => {
    const out: Partial<Record<OverlayId, Series | ReturnType<typeof boll>>> = {};
    if (overlays.has('ma5')) out.ma5 = sma(closes, 5);
    if (overlays.has('ma10')) out.ma10 = sma(closes, 10);
    if (overlays.has('ma20')) out.ma20 = sma(closes, 20);
    if (overlays.has('ma60')) out.ma60 = sma(closes, 60);
    if (overlays.has('ema12')) out.ema12 = ema(closes, 12);
    if (overlays.has('ema26')) out.ema26 = ema(closes, 26);
    if (overlays.has('boll')) out.boll = boll(closes, 20, 2);
    return out;
  }, [closes, overlays]);

  const macdSeries = useMemo(() => (showMacd ? macd(closes) : null), [closes, showMacd]);
  const bollSeries = useMemo(
    () => (series.boll as ReturnType<typeof boll> | undefined) ?? null,
    [series],
  );
  const [hoverX, setHoverX] = useState(0);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const ro = new ResizeObserver(() => {
      setSize({ w: host.clientWidth, h: host.clientHeight });
    });
    ro.observe(host);
    setSize({ w: host.clientWidth, h: host.clientHeight });
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || size.w === 0 || size.h === 0) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.w * dpr);
    canvas.height = Math.round(size.h * dpr);
    canvas.style.width = `${size.w}px`;
    canvas.style.height = `${size.h}px`;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size.w, size.h);

    const labelColor = cssVar('--dsw-alias-label-tertiary', '#959da6');
    const gridColor = cssVar('--dsw-alias-border-l2', '#e5e8ef');
    const strongLabel = cssVar('--dsw-alias-label-primary', '#1d2129');

    const PAD = { top: 10, right: 64, bottom: 20, left: 6 };
    const plotL = PAD.left;
    const plotR = size.w - PAD.right;
    const plotW = Math.max(10, plotR - plotL);
    const usableH = Math.max(40, size.h - PAD.top - PAD.bottom);
    const gap = 8;
    const volH = showVolume ? Math.max(40, usableH * 0.16) : 0;
    const macdH = showMacd ? Math.max(46, usableH * 0.2) : 0;
    const mainH = Math.max(40, usableH - volH - macdH - (showVolume ? gap : 0) - (showMacd ? gap : 0));
    const mainTop = PAD.top;
    const mainBottom = mainTop + mainH;
    const volTop = mainBottom + gap;
    const volBottom = volTop + volH;
    const macdTop = (showVolume ? volBottom : mainBottom) + (showMacd ? gap : 0);
    const macdBottom = macdTop + macdH;

    if (bars.length === 0) {
      ctx.fillStyle = labelColor;
      ctx.font = '13px system-ui, -apple-system, "Segoe UI", sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('当前周期没有数据', size.w / 2, size.h / 2);
      return;
    }

    const n = bars.length;
    const step = plotW / n;
    const xOf = (i: number) => plotL + step * (i + 0.5);

    // ── 主图纵轴范围（含已勾选的 BOLL 轨，避免轨道被裁掉）──
    let min = Infinity;
    let max = -Infinity;
    for (const b of bars) {
      if (b.low < min) min = b.low;
      if (b.high > max) max = b.high;
    }
    const bollBand = series.boll as ReturnType<typeof boll> | undefined;
    if (bollBand) {
      for (const arr of [bollBand.upper, bollBand.lower]) {
        for (const v of arr) {
          if (v == null) continue;
          if (v < min) min = v;
          if (v > max) max = v;
        }
      }
    }
    if (!Number.isFinite(min) || !Number.isFinite(max)) return;
    if (min === max) {
      min -= 1;
      max += 1;
    }
    const padY = (max - min) * 0.08;
    min -= padY;
    max += padY;
    const yOf = (v: number) => mainBottom - ((v - min) / (max - min)) * mainH;

    // ── 网格与价格刻度 ──
    ctx.font = '11px ui-monospace, Menlo, Consolas, monospace';
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    const gridCount = 5;
    for (let i = 0; i <= gridCount; i += 1) {
      const v = min + ((max - min) * i) / gridCount;
      const y = yOf(v);
      ctx.strokeStyle = gridColor;
      ctx.globalAlpha = 0.5;
      ctx.beginPath();
      ctx.moveTo(plotL, y);
      ctx.lineTo(plotR, y);
      ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.fillStyle = labelColor;
      ctx.fillText(fmtPrice(v), plotR + 6, y);
    }

    // ── 蜡烛 ──
    const bodyW = Math.max(1, Math.min(13, step * 0.66));
    for (let i = 0; i < n; i += 1) {
      const b = bars[i];
      const up = b.close >= b.open;
      const color = up ? UP : DOWN;
      const cx = xOf(i);
      ctx.strokeStyle = color;
      ctx.fillStyle = color;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(cx, yOf(b.high));
      ctx.lineTo(cx, yOf(b.low));
      ctx.stroke();
      const yOpen = yOf(b.open);
      const yClose = yOf(b.close);
      const top = Math.min(yOpen, yClose);
      const h = Math.max(1, Math.abs(yClose - yOpen));
      if (up) {
        // 阳线中空（A 股习惯）：描边 + 背景色填充，便于密集区分辨。
        ctx.strokeRect(cx - bodyW / 2, top, bodyW, h);
      } else {
        ctx.fillRect(cx - bodyW / 2, top, bodyW, h);
      }
    }

    // ── 均线 / BOLL ──
    const drawSeries = (values: Series, color: string, width = 1.2) => {
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.beginPath();
      let started = false;
      for (let i = 0; i < n; i += 1) {
        const v = values[i];
        if (v == null) {
          started = false;
          continue;
        }
        const x = xOf(i);
        const y = yOf(v);
        if (!started) {
          ctx.moveTo(x, y);
          started = true;
        } else {
          ctx.lineTo(x, y);
        }
      }
      ctx.stroke();
    };
    for (const meta of OVERLAY_META) {
      if (!overlays.has(meta.id) || meta.id === 'boll') continue;
      const values = series[meta.id] as Series | undefined;
      if (values) drawSeries(values, meta.color);
    }
    if (bollSeries) {
      drawSeries(bollSeries.upper, OVERLAY_META[6].color, 1);
      drawSeries(bollSeries.middle, '#e8a33d', 1);
      drawSeries(bollSeries.lower, OVERLAY_META[6].color, 1);
    }

    // ── 成交量 ──
    if (showVolume && volH > 0) {
      const maxVol = bars.reduce((m, b) => Math.max(m, b.volume || 0), 0) || 1;
      for (let i = 0; i < n; i += 1) {
        const b = bars[i];
        const h = Math.max(0.6, ((b.volume || 0) / maxVol) * (volH - 6));
        ctx.fillStyle = b.close >= b.open ? 'rgba(213,73,65,.55)' : 'rgba(43,164,113,.55)';
        ctx.fillRect(xOf(i) - bodyW / 2, volBottom - h, bodyW, h);
      }
      ctx.fillStyle = labelColor;
      ctx.textAlign = 'left';
      ctx.fillText(`VOL ${fmtVolume(bars[n - 1].volume || 0)}`, plotL + 2, volTop + 8);
      ctx.strokeStyle = gridColor;
      ctx.globalAlpha = 0.5;
      ctx.beginPath();
      ctx.moveTo(plotL, volBottom + 0.5);
      ctx.lineTo(plotR, volBottom + 0.5);
      ctx.stroke();
      ctx.globalAlpha = 1;
    }

    // ── MACD ──
    if (showMacd && macdSeries && macdH > 0) {
      const vals: number[] = [];
      for (const arr of [macdSeries.dif, macdSeries.dea, macdSeries.hist]) {
        for (const v of arr) if (v != null) vals.push(v);
      }
      const absMax = vals.reduce((m, v) => Math.max(m, Math.abs(v)), 0) || 1;
      const mid = (macdTop + macdBottom) / 2;
      const scale = (macdH / 2 - 4) / absMax;
      const yMacd = (v: number) => mid - v * scale;
      ctx.strokeStyle = gridColor;
      ctx.globalAlpha = 0.5;
      ctx.beginPath();
      ctx.moveTo(plotL, mid);
      ctx.lineTo(plotR, mid);
      ctx.stroke();
      ctx.globalAlpha = 1;
      for (let i = 0; i < n; i += 1) {
        const v = macdSeries.hist[i];
        if (v == null) continue;
        ctx.fillStyle = v >= 0 ? 'rgba(213,73,65,.6)' : 'rgba(43,164,113,.6)';
        const y = yMacd(v);
        ctx.fillRect(xOf(i) - bodyW / 2, Math.min(y, mid), bodyW, Math.max(0.8, Math.abs(mid - y)));
      }
      const drawMacdLine = (values: Series, color: string) => {
        ctx.strokeStyle = color;
        ctx.lineWidth = 1.2;
        ctx.beginPath();
        let started = false;
        for (let i = 0; i < n; i += 1) {
          const v = values[i];
          if (v == null) {
            started = false;
            continue;
          }
          if (!started) {
            ctx.moveTo(xOf(i), yMacd(v));
            started = true;
          } else {
            ctx.lineTo(xOf(i), yMacd(v));
          }
        }
        ctx.stroke();
      };
      drawMacdLine(macdSeries.dif, '#3a6df0');
      drawMacdLine(macdSeries.dea, '#e8a33d');
      ctx.fillStyle = labelColor;
      ctx.textAlign = 'left';
      ctx.fillText('MACD(12,26,9)', plotL + 2, macdTop + 8);
      ctx.fillStyle = '#3a6df0';
      ctx.fillText('DIF', plotL + 92, macdTop + 8);
      ctx.fillStyle = '#e8a33d';
      ctx.fillText('DEA', plotL + 120, macdTop + 8);
    }

    // ── 时间轴 ──
    ctx.fillStyle = labelColor;
    ctx.textBaseline = 'top';
    const ticks = Math.min(6, Math.max(2, Math.floor(plotW / 130)));
    for (let i = 0; i <= ticks; i += 1) {
      const idx = Math.min(n - 1, Math.round((n - 1) * (i / ticks)));
      const label = fmtTime(bars[idx].time, interval);
      const x = xOf(idx);
      ctx.textAlign = i === 0 ? 'left' : i === ticks ? 'right' : 'center';
      ctx.fillText(label, Math.min(plotR, Math.max(plotL, x)), size.h - PAD.bottom + 5);
    }

    // ── 十字光标 ──
    if (hover != null && hover >= 0 && hover < n) {
      const b = bars[hover];
      const x = xOf(hover);
      ctx.strokeStyle = labelColor;
      ctx.globalAlpha = 0.7;
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(x, mainTop);
      ctx.lineTo(x, size.h - PAD.bottom);
      ctx.stroke();
      if (b.close >= min && b.close <= max) {
        const y = yOf(b.close);
        ctx.beginPath();
        ctx.moveTo(plotL, y);
        ctx.lineTo(plotR, y);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;

      // 右侧悬浮价格标签
      const y = yOf(b.close);
      ctx.fillStyle = b.close >= b.open ? UP : DOWN;
      const text = fmtPrice(b.close);
      ctx.font = '11px ui-monospace, Menlo, Consolas, monospace';
      const w = ctx.measureText(text).width + 10;
      ctx.fillRect(plotR + 1, y - 8, w, 16);
      ctx.fillStyle = '#fff';
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.fillText(text, plotR + 6, y);
    }

    // 高亮最后一个收盘价（不 hover 时也有参照）
    if (hover == null) {
      const last = bars[n - 1];
      const color = last.close >= last.open ? UP : DOWN;
      const y = yOf(last.close);
      ctx.setLineDash([2, 4]);
      ctx.strokeStyle = color;
      ctx.globalAlpha = 0.55;
      ctx.beginPath();
      ctx.moveTo(plotL, y);
      ctx.lineTo(plotR, y);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    // 保留 strongLabel 引用，避免未使用告警（plot 区域标题可后续扩展）
    void strongLabel;
  }, [bars, size, overlays, series, showVolume, showMacd, macdSeries, hover, interval]);

  const hoverBar = hover != null && hover >= 0 && hover < bars.length ? bars[hover] : null;

  return (
    <div className="dw-chart" ref={hostRef} onMouseLeave={() => setHover(null)}>
      <canvas
        ref={canvasRef}
        style={{ display: 'block', width: '100%', height: '100%' }}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const plotL = 6;
          const plotW = Math.max(10, rect.width - 6 - 64);
          const ratio = (e.clientX - rect.left - plotL) / plotW;
          if (!Number.isFinite(ratio) || bars.length === 0) return;
          const idx = Math.max(0, Math.min(bars.length - 1, Math.floor(ratio * bars.length)));
          setHover(idx);
          setHoverX(e.clientX - rect.left);
        }}
      />
      {hoverBar && (
        <div
          className="dw-tip"
          style={{
            left: Math.min(Math.max(hoverX + 12, 8), Math.max(8, size.w - 250)),
            top: 10,
          }}
        >
          <div className="dw-tip-time">{fmtTime(hoverBar.time, interval)}</div>
          <div>开 <b>{fmtPrice(hoverBar.open)}</b> 高 <b>{fmtPrice(hoverBar.high)}</b></div>
          <div>低 <b>{fmtPrice(hoverBar.low)}</b> 收 <b>{fmtPrice(hoverBar.close)}</b></div>
          <div>量 <b>{fmtVolume(hoverBar.volume)}</b></div>
          {OVERLAY_META.filter((m) => m.id !== 'boll' && overlays.has(m.id)).length > 0 ? (
            <div>
              {OVERLAY_META.filter((m) => m.id !== 'boll' && overlays.has(m.id)).map((m) => {
                const seriesValues = series[m.id] as Series | undefined;
                const v = seriesValues ? seriesValues[hover ?? 0] : null;
                return v == null ? null : (
                  <span key={m.id} style={{ color: m.color, marginRight: 8 }}>
                    {m.label} {fmtPrice(v)}
                  </span>
                );
              })}
            </div>
          ) : null}
          {overlays.has('boll') && bollSeries ? (
            <div style={{ color: OVERLAY_META[6].color }}>
              BOLL {fmtPrice(bollSeries.upper[hover ?? 0] ?? NaN)} /{' '}
              {fmtPrice(bollSeries.middle[hover ?? 0] ?? NaN)} /{' '}
              {fmtPrice(bollSeries.lower[hover ?? 0] ?? NaN)}
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}
