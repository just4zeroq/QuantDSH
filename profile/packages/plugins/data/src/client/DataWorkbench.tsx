import React, { useEffect, useMemo, useState } from 'react';
import type { DatasetSummary, Interval } from '../domain';
import { intervalMs, INTERVALS, MARKET_LABEL, summarizeDataset } from '../domain';
import { coverageTone, formatPercent, toDetailProps, toRowProps } from './dataset-presentation';
import { useDatahubWorkbench, type DatahubClient } from './use-datahub';
import type { DatasetWire } from '../datahub/client';
import { KlineChart, OVERLAY_META, type ChartBar, type OverlayId } from './KlineChart';

// UI 只消费 domain 层的纯逻辑（覆盖率 / 新鲜度 / 契约口径的唯一实现），
// 与 Agent 工具、回测加载器共用同一套判定 —— 避免"页面看到的和引擎认为的不一样"。
//
// 数据来源：Rust 守护进程（quantdsh-datahub）通过 HTTP 提供。
// 页面是众多只读消费者之一；守护进程是唯一写者（DuckDB 跨进程只允许一个写者）。
//
// 版式契约（产品决定，勿随意改回）：
//   顶层只有三个页面：行情 / 数据 / 管理。
//   行情 = K 线 + 可勾选指标；数据 = SQL 查询 + CSV 导入；管理 = 选交易对、看概览、清数据。
//   不再有常驻左栏、不再有二级 tab —— 每个页面自洽，切页即换全部内容。

const TONE_COLOR: Record<string, string> = {
  ok: 'var(--dsw-alias-status-success, #2ba471)',
  warn: 'var(--dsw-alias-status-warning, #e37318)',
  alert: 'var(--dsw-alias-status-error, #d54941)',
  muted: 'var(--dsw-alias-label-tertiary, #959da6)',
};

/** 涨红跌绿（中国市场惯例，与欧美相反）。 */
const UP_COLOR = TONE_COLOR.alert;
const DOWN_COLOR = TONE_COLOR.ok;

type TabId = 'market' | 'data' | 'manage';

const TABS: { id: TabId; label: string; hint: string }[] = [
  { id: 'market', label: '行情', hint: 'K 线与指标' },
  { id: 'data', label: '数据', hint: '查询与导入' },
  { id: 'manage', label: '管理', hint: '概览与清理' },
];

const styles = `
.dw-root{height:100%;min-height:0;display:flex;flex-direction:column;overflow:hidden;width:100%;font:inherit;color:var(--dsw-alias-label-primary,#1d2129);background:var(--dsw-alias-bg-base,#fff)}
.dw-tabs{display:flex;gap:4px;padding:0 16px;border-bottom:1px solid var(--dsw-alias-border-l2,#dce2e9);flex:none}
.dw-tab{padding:11px 16px;font-size:13px;cursor:pointer;border-bottom:2px solid transparent;color:var(--dsw-alias-label-secondary,#657185);white-space:nowrap;user-select:none}
.dw-tab.active{color:var(--dsw-alias-label-primary,#1d2129);border-bottom-color:var(--dsw-alias-brand,#3a6df0);font-weight:600}
.dw-tab small{display:block;font-size:11px;font-weight:400;color:var(--dsw-alias-label-tertiary,#959da6)}
.dw-body{padding:0;overflow:hidden;min-height:0;flex:1;display:flex;flex-direction:column}
.dw-scroll{padding:16px;overflow:auto;min-height:0;flex:1}
.dw-card{border:1px solid var(--dsw-alias-border-l2,#dce2e9);border-radius:10px;padding:14px 16px;margin-bottom:14px}
.dw-card h3{margin:0 0 10px;font-size:13px;font-weight:600}
.dw-kv{display:flex;justify-content:space-between;gap:12px;padding:6px 0;border-bottom:1px dashed var(--dsw-alias-border-l2,#dce2e9);font-size:13px}
.dw-kv:last-child{border-bottom:0}
.dw-kv span:first-child{color:var(--dsw-alias-label-secondary,#657185)}
.dw-note{margin:12px 0 0;font-size:12px;color:var(--dsw-alias-label-tertiary,#959da6);line-height:1.5}
.dw-loading{padding:24px 16px;font-size:13px;color:var(--dsw-alias-label-secondary,#657185)}
.dw-empty{padding:24px 16px;font-size:13px;color:var(--dsw-alias-label-secondary,#657185);line-height:1.6}
.dw-empty code{background:var(--dsw-alias-bg-layer-1,#f2f3f5);padding:1px 5px;border-radius:4px}
.dw-btn{font:inherit;font-size:12px;padding:5px 12px;border-radius:7px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129);cursor:pointer}
.dw-btn:hover{background:var(--dsw-alias-bg-layer-1,#f2f3f5)}
.dw-btn.primary{background:var(--dsw-alias-brand,#3a6df0);border-color:var(--dsw-alias-brand,#3a6df0);color:#fff}
.dw-btn.danger{border-color:var(--dsw-alias-status-error,#d54941);color:var(--dsw-alias-status-error,#d54941)}
.dw-btn:disabled{opacity:.5;cursor:not-allowed}
.dw-rowflex{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:12px}
.dw-form{display:grid;grid-template-columns:110px 1fr;gap:8px 12px;align-items:center;margin-bottom:12px}
.dw-form label{font-size:13px;color:var(--dsw-alias-label-secondary,#657185);text-align:right}
.dw-input,.dw-select,.dw-textarea{font:inherit;font-size:13px;padding:6px 8px;border-radius:7px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129);width:100%;box-sizing:border-box}
.dw-textarea{min-height:120px;font-family:ui-monospace,Menlo,Consolas,monospace;resize:vertical}
.dw-table{width:100%;border-collapse:collapse;font-size:12px;margin-top:8px}
.dw-table th,.dw-table td{border:1px solid var(--dsw-alias-border-l2,#dce2e9);padding:6px 8px;text-align:left}
.dw-table th{background:var(--dsw-alias-bg-layer-1,#f2f3f5);position:sticky;top:0;z-index:1}
.dw-table tbody tr:hover{background:var(--dsw-alias-bg-layer-1,#f2f3f5)}
.dw-table-wrap{max-height:420px;overflow:auto;border:1px solid var(--dsw-alias-border-l2,#dce2e9);border-radius:8px}
.dw-find{display:flex;gap:8px;padding:6px 8px;border-radius:6px;font-size:12px;margin-bottom:4px;align-items:center}
.dw-find.error{background:rgba(213,73,65,.08);color:var(--dsw-alias-status-error,#d54941)}
.dw-find.warn{background:rgba(227,115,24,.08);color:var(--dsw-alias-status-warning,#e37318)}
.dw-find.ok{background:rgba(43,164,113,.08);color:var(--dsw-alias-status-success,#2ba471)}
.dw-toolbar{display:flex;gap:10px;align-items:center;flex-wrap:wrap;padding:10px 16px;border-bottom:1px solid var(--dsw-alias-border-l2,#dce2e9);flex:none}
.dw-toolbar .dw-select{width:auto;min-width:170px}
.dw-kpis{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;margin-bottom:14px}
.dw-kpi{border:1px solid var(--dsw-alias-border-l2,#dce2e9);border-radius:10px;padding:12px 14px}
.dw-kpi b{display:block;font-size:19px;font-weight:600;line-height:1.3}
.dw-kpi span{font-size:12px;color:var(--dsw-alias-label-secondary,#657185)}
.dw-mono{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px}
.dw-right{text-align:right}
.dw-empty-cell{padding:20px 8px;text-align:center;color:var(--dsw-alias-label-tertiary,#959da6)}
.dw-pills{display:flex;gap:4px;flex-wrap:wrap}
.dw-pill{font:inherit;font-size:12px;padding:4px 10px;border-radius:999px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:transparent;color:var(--dsw-alias-label-secondary,#657185);cursor:pointer;line-height:1.2}
.dw-pill.active{background:var(--dsw-alias-bg-layer-2,#e8f0ff);border-color:var(--dsw-alias-brand,#3a6df0);color:var(--dsw-alias-label-primary,#1d2129);font-weight:600}
.dw-pill:disabled{opacity:.35;cursor:not-allowed}
.dw-chips{display:flex;gap:6px;flex-wrap:wrap}
.dw-chip{display:inline-flex;align-items:center;gap:5px;font-size:12px;padding:4px 9px;border-radius:999px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);cursor:pointer;user-select:none;color:var(--dsw-alias-label-secondary,#657185);background:transparent}
.dw-chip.on{color:var(--dsw-alias-label-primary,#1d2129);border-color:currentColor}
.dw-chip i{width:7px;height:7px;border-radius:50%;display:inline-block}
.dw-chart{position:relative;flex:1;min-height:0;width:100%;cursor:crosshair}
.dw-tip{position:absolute;pointer-events:none;z-index:5;background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#dce2e9);border-radius:8px;padding:8px 10px;font-size:11px;line-height:1.7;color:var(--dsw-alias-label-secondary,#657185);box-shadow:0 6px 20px rgba(0,0,0,.12);min-width:180px}
.dw-tip b{color:var(--dsw-alias-label-primary,#1d2129);font-family:ui-monospace,Menlo,Consolas,monospace}
.dw-tip-time{color:var(--dsw-alias-label-primary,#1d2129);font-weight:600;margin-bottom:2px}
.dw-price{font-size:15px;font-weight:600;font-family:ui-monospace,Menlo,Consolas,monospace}
.dw-muted{color:var(--dsw-alias-label-tertiary,#959da6);font-size:12px}
.dw-danger-zone{border:1px solid rgba(213,73,65,.35);border-radius:10px;padding:14px 16px;margin-bottom:14px;background:rgba(213,73,65,.03)}
.dw-danger-zone h3{color:var(--dsw-alias-status-error,#d54941)}
`;

export function DataWorkbench({ nowTs }: { nowTs: number }) {
  // 页面自己维持时间基准：面板只挂载一次，用传入值会让"新鲜度"永久停在上次进入时刻。
  const [now, setNow] = useState(nowTs);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const state = useDatahubWorkbench(now);
  const [tab, setTab] = useState<TabId>('market');

  const wire = state.kind === 'ready' ? state.meta.wire : [];
  const datasets = state.kind === 'ready' ? state.meta.datasets : null;
  const summaries = useMemo(
    () => (datasets ?? []).map((d) => summarizeDataset(d, now)),
    [datasets, now],
  );

  return (
    <div className="dw-root">
      <style>{styles}</style>
      <div className="dw-tabs" role="tablist">
        {TABS.map((t) => (
          <div
            key={t.id}
            role="tab"
            aria-selected={t.id === tab}
            className={`dw-tab${t.id === tab ? ' active' : ''}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
            <small>{t.hint}</small>
          </div>
        ))}
      </div>
      <div className="dw-body">
        {state.kind !== 'ready' ? (
          <div className="dw-scroll">
            <StatusPanel state={state} />
          </div>
        ) : tab === 'market' ? (
          <MarketPageView wire={wire} client={state.client} />
        ) : tab === 'data' ? (
          <DataPageView wire={wire} client={state.client} onChanged={state.reload} />
        ) : (
          <ManagePageView
            wire={wire}
            summaries={summaries}
            client={state.client}
            now={now}
            onChanged={state.reload}
          />
        )}
      </div>
    </div>
  );
}

function StatusPanel({ state }: { state: ReturnType<typeof useDatahubWorkbench> }) {
  if (state.kind === 'loading') return <p className="dw-loading">正在装载数据集…</p>;
  if (state.kind === 'unreachable') {
    return (
      <div className="dw-card">
        <h3 style={{ color: TONE_COLOR.alert }}>数据服务未连接</h3>
        <p className="dw-empty" style={{ padding: 0 }}>
          {state.message}。守护进程随应用启动；开发态可手动运行：
          <br />
          <code>cargo run -p quantdsh-datahub</code>
        </p>
        <button className="dw-btn" onClick={state.reload}>重试</button>
      </div>
    );
  }
  if (state.kind === 'error') {
    return (
      <div className="dw-card">
        <h3 style={{ color: TONE_COLOR.alert }}>读取失败</h3>
        <p className="dw-empty" style={{ padding: 0 }}>{state.message}</p>
        {/* 令牌轮换是自愈场景：给一个显式重试，别让用户只能重启应用。 */}
        <button className="dw-btn" onClick={state.reload}>重试</button>
      </div>
    );
  }
  return null;
}

// ───────────────────────────── 共用：标的 / 周期选择 ─────────────────────────────

function useSymbolPicker(wire: DatasetWire[]) {
  const symbols = useMemo(
    () => [...new Set(wire.map((w) => w.symbol))].sort((a, b) => a.localeCompare(b)),
    [wire],
  );
  const [symbol, setSymbol] = useState('');

  useEffect(() => {
    if (symbols.length === 0) {
      if (symbol) setSymbol('');
      return;
    }
    if (!symbol || !symbols.includes(symbol)) setSymbol(symbols[0]);
  }, [symbols, symbol]);

  const intervals = useMemo(
    () =>
      INTERVALS.filter((iv) => wire.some((w) => w.symbol === symbol && w.interval === iv)),
    [wire, symbol],
  );

  const [interval, setInterval] = useState<Interval | ''>('');

  useEffect(() => {
    if (intervals.length === 0) {
      if (interval) setInterval('');
      return;
    }
    if (!interval || !intervals.includes(interval)) setInterval(intervals[0]);
  }, [intervals, interval]);

  const dataset = useMemo(
    () => wire.find((w) => w.symbol === symbol && w.interval === interval) ?? null,
    [wire, symbol, interval],
  );

  return { symbols, symbol, setSymbol, intervals, interval, setInterval, dataset };
}

function useBars(
  client: DatahubClient,
  dataset: DatasetWire | null,
  count: number,
  nonce: number,
) {
  const [bars, setBars] = useState<ChartBar[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!dataset) {
      setBars([]);
      return;
    }
    let cancelled = false;
    const step = intervalMs(dataset.interval as Interval);
    // /api/bars 是 ORDER BY ts ASC LIMIT —— 必须带窗口，否则拿到的是最老的 bar。
    const anchor = dataset.as_of ?? Date.now();
    const to = anchor + step;
    const from = to - step * (Math.max(1, count) + 1);
    setLoading(true);
    setErr(null);
    void (async () => {
      try {
        const rows = await client.bars({
          symbol: dataset.symbol,
          interval: dataset.interval,
          namespace: dataset.namespace,
          from,
          to,
          limit: Math.max(1, count),
        });
        if (cancelled) return;
        const mapped: ChartBar[] = rows
          .map((r) => ({
            time: Number(r.ts),
            open: Number(r.open),
            high: Number(r.high),
            low: Number(r.low),
            close: Number(r.close),
            volume: Number(r.volume ?? 0),
          }))
          .filter((b) => Number.isFinite(b.time) && Number.isFinite(b.close))
          .sort((a, b) => a.time - b.time);
        setBars(mapped);
      } catch (cause) {
        if (!cancelled) setErr(errMsg(cause));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, dataset?.dataset_key, dataset?.as_of, count, nonce]);

  return { bars, loading, err };
}

// ───────────────────────────── 页面一：行情 ─────────────────────────────

function MarketPageView({ wire, client }: { wire: DatasetWire[]; client: DatahubClient }) {
  const picker = useSymbolPicker(wire);
  const [count, setCount] = useState(500);
  const [nonce, setNonce] = useState(0);
  const [overlays, setOverlays] = useState<Set<OverlayId>>(
    () => new Set<OverlayId>(['ma5', 'ma10', 'ma20', 'boll']),
  );
  const [showVolume, setShowVolume] = useState(true);
  const [showMacd, setShowMacd] = useState(false);

  const { bars, loading, err } = useBars(client, picker.dataset, count, nonce);

  const last = bars.length > 0 ? bars[bars.length - 1] : null;
  const prev = bars.length > 1 ? bars[bars.length - 2] : null;
  const change = last && prev && prev.close !== 0 ? (last.close - prev.close) / prev.close : null;
  const tone = change == null || change === 0 ? TONE_COLOR.muted : change > 0 ? UP_COLOR : DOWN_COLOR;

  const toggleOverlay = (id: OverlayId) => {
    setOverlays((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  if (wire.length === 0) {
    return (
      <div className="dw-scroll">
        <EmptyCatalog />
      </div>
    );
  }

  return (
    <>
      <div className="dw-toolbar">
        <select
          className="dw-select"
          value={picker.symbol}
          onChange={(e) => picker.setSymbol(e.target.value)}
        >
          {picker.symbols.map((s) => (
            <option key={s} value={s}>{s}</option>
          ))}
        </select>
        <div className="dw-pills">
          {picker.intervals.map((iv) => (
            <button
              key={iv}
              className={`dw-pill${iv === picker.interval ? ' active' : ''}`}
              onClick={() => picker.setInterval(iv)}
            >
              {iv}
            </button>
          ))}
        </div>
        <div className="dw-pills">
          {[200, 500, 1000, 2000].map((c) => (
            <button
              key={c}
              className={`dw-pill${c === count ? ' active' : ''}`}
              onClick={() => setCount(c)}
            >
              {c} 根
            </button>
          ))}
        </div>
        <button className="dw-btn" onClick={() => setNonce((n) => n + 1)} disabled={loading}>
          {loading ? '加载中…' : '刷新'}
        </button>
        {last && (
          <span style={{ display: 'inline-flex', gap: 8, alignItems: 'baseline' }}>
            <span className="dw-price" style={{ color: tone }}>{fmtPrice(last.close)}</span>
            <span className="dw-mono" style={{ color: tone }}>
              {change == null ? '—' : `${change > 0 ? '+' : ''}${(change * 100).toFixed(2)}%`}
            </span>
          </span>
        )}
      </div>

      <div className="dw-toolbar" style={{ borderBottom: '1px solid var(--dsw-alias-border-l2,#dce2e9)' }}>
        <span className="dw-muted">指标</span>
        <div className="dw-chips">
          {OVERLAY_META.map((m) => (
            <label
              key={m.id}
              className={`dw-chip${overlays.has(m.id) ? ' on' : ''}`}
              style={overlays.has(m.id) ? { color: m.color, borderColor: m.color } : undefined}
            >
              <input
                type="checkbox"
                checked={overlays.has(m.id)}
                onChange={() => toggleOverlay(m.id)}
                style={{ display: 'none' }}
              />
              <i style={{ background: m.color }} />
              {m.label}
            </label>
          ))}
          <label className={`dw-chip${showVolume ? ' on' : ''}`}>
            <input
              type="checkbox"
              checked={showVolume}
              onChange={() => setShowVolume((v) => !v)}
              style={{ display: 'none' }}
            />
            <i style={{ background: 'rgba(122,134,153,.8)' }} />
            成交量
          </label>
          <label className={`dw-chip${showMacd ? ' on' : ''}`}>
            <input
              type="checkbox"
              checked={showMacd}
              onChange={() => setShowMacd((v) => !v)}
              style={{ display: 'none' }}
            />
            <i style={{ background: 'rgba(58,109,240,.8)' }} />
            MACD
          </label>
        </div>
        <span className="dw-muted" style={{ marginLeft: 'auto' }}>
          {picker.dataset ? `${picker.dataset.symbol} · ${picker.dataset.interval} · ${bars.length} 根` : '未选择数据集'}
        </span>
      </div>

      {err && <div className="dw-find error" style={{ margin: '10px 16px' }}>{err}</div>}
      <KlineChart
        bars={bars}
        interval={String(picker.interval || '1h')}
        overlays={overlays}
        showVolume={showVolume}
        showMacd={showMacd}
      />
    </>
  );
}

// ───────────────────────────── 页面二：数据（查询 + 导入） ─────────────────────────────

function DataPageView({
  wire,
  client,
  onChanged,
}: {
  wire: DatasetWire[];
  client: DatahubClient;
  onChanged: () => void;
}) {
  return (
    <div className="dw-scroll">
      <SqlSection client={client} onChanged={onChanged} />
      <ImportSection wire={wire} client={client} onChanged={onChanged} />
    </div>
  );
}

function SqlSection({ client, onChanged }: { client: DatahubClient; onChanged: () => void }) {
  const [sql, setSql] = useState('SELECT symbol, "interval", count(*) AS bars, min(ts) AS from_ts, max(ts) AS to_ts FROM bars GROUP BY 1,2 ORDER BY bars DESC');
  const [maxRows, setMaxRows] = useState(500);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<null | {
    status: 'ok' | 'blocked' | 'error';
    columns: string[];
    rows: Record<string, unknown>[];
    truncated: boolean;
    duration_ms: number;
    reason?: string;
    message?: string;
  }>(null);
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setBusy(true);
    setErr(null);
    try {
      const r = await client.query(sql, maxRows);
      setResult(r);
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  const samples = [
    { label: '数据集统计', sql },
    { label: '最近 50 根', sql: 'SELECT * FROM bars ORDER BY ts DESC LIMIT 50' },
    { label: '表清单', sql: 'SELECT * FROM information_schema.tables' },
  ];

  return (
    <div className="dw-card">
      <h3>SQL 查询（只读 · 单语句 · 行数上限）</h3>
      <textarea
        className="dw-textarea"
        style={{ minHeight: 96 }}
        value={sql}
        onChange={(e) => setSql(e.target.value)}
        spellCheck={false}
      />
      <div className="dw-rowflex" style={{ marginTop: 8 }}>
        <button className="dw-btn primary" disabled={busy || !sql.trim()} onClick={run}>
          {busy ? '执行中…' : '执行'}
        </button>
        <span className="dw-muted">上限</span>
        <input
          className="dw-input"
          style={{ width: 100 }}
          type="number"
          value={maxRows}
          onChange={(e) => setMaxRows(Number(e.target.value))}
        />
        <span className="dw-muted" style={{ marginLeft: 'auto' }}>示例</span>
        {samples.map((s, i) => (
          <button key={i} className="dw-btn" onClick={() => setSql(s.sql)}>{s.label}</button>
        ))}
      </div>
      {err && <div className="dw-find error">{err}</div>}
      {result && (
        <>
          <div
            className={`dw-find ${result.status === 'ok' ? 'ok' : result.status === 'blocked' ? 'warn' : 'error'}`}
          >
            {result.status === 'ok'
              ? `成功 · ${result.rows.length} 行 · ${result.duration_ms}ms${result.truncated ? ' · 已截断' : ''}`
              : result.status === 'blocked'
                ? `已拦截：${result.reason ?? ''}`
                : `错误：${result.message ?? ''}`}
          </div>
          {result.status === 'ok' && result.columns.length > 0 && (
            <div className="dw-table-wrap">
              <table className="dw-table">
                <thead>
                  <tr>{result.columns.map((c) => <th key={c}>{c}</th>)}</tr>
                </thead>
                <tbody>
                  {result.rows.map((row, i) => (
                    <tr key={i}>
                      {result.columns.map((c) => <td key={c} className="dw-mono">{fmtCell(row[c])}</td>)}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  );
}

function ImportSection({
  wire,
  client,
  onChanged,
}: {
  wire: DatasetWire[];
  client: DatahubClient;
  onChanged: () => void;
}) {
  const [form, setForm] = useState({
    dataset_key: '',
    symbol: '',
    interval: '1m' as Interval,
    file: 'manual.csv',
    text: '',
    namespace: 'imported',
    max_bad_ratio: '',
  });
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<null | {
    batch_id: string;
    rows_total: number;
    rows_ok: number;
    rows_bad: number;
    gate_passed: boolean;
    written: number;
    findings: { kind: string; severity: string; message: string; count: number }[];
    bad_samples: string[];
  }>(null);
  const [err, setErr] = useState<string | null>(null);

  const applyDataset = (key: string) => {
    const target = wire.find((w) => w.dataset_key === key);
    setForm((f) => ({
      ...f,
      dataset_key: key,
      symbol: target?.symbol ?? f.symbol,
      interval: (target?.interval as Interval) ?? f.interval,
    }));
  };

  async function pickFile(file: File | null) {
    if (!file) return;
    try {
      const text = await file.text();
      setForm((f) => ({ ...f, file: file.name, text }));
    } catch (e) {
      setErr(errMsg(e));
    }
  }

  async function doImport() {
    setBusy(true);
    setErr(null);
    setResult(null);
    try {
      const r = await client.importCsv({
        dataset_key: form.dataset_key,
        symbol: form.symbol,
        interval: form.interval,
        file: form.file,
        text: form.text,
        namespace: form.namespace || undefined,
        max_bad_ratio: form.max_bad_ratio ? Number(form.max_bad_ratio) : undefined,
      });
      setResult(r);
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  async function promote() {
    setBusy(true);
    setErr(null);
    try {
      const r = await client.promote({
        dataset_key: form.dataset_key,
        symbol: form.symbol,
        interval: form.interval,
      });
      setResult({
        batch_id: 'promote',
        rows_total: r.moved,
        rows_ok: r.moved,
        rows_bad: 0,
        gate_passed: true,
        written: r.moved,
        findings: [],
        bad_samples: [],
      });
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dw-card">
      <h3>CSV 导入（先进 imported 命名空间，通过门禁后可提升 bronze）</h3>
      <div className="dw-form">
        <label>参照数据集</label>
        <select className="dw-select" value={form.dataset_key} onChange={(e) => applyDataset(e.target.value)}>
          <option value="">不指定（手动填写）</option>
          {wire.map((w) => (
            <option key={w.dataset_key} value={w.dataset_key}>
              {w.symbol} · {w.interval} · {w.namespace}
            </option>
          ))}
        </select>
        <label>数据集键</label>
        <input className="dw-input" value={form.dataset_key} onChange={(e) => setForm({ ...form, dataset_key: e.target.value })} placeholder="如 binance:BTCUSDT:1m" />
        <label>标的</label>
        <input className="dw-input" value={form.symbol} onChange={(e) => setForm({ ...form, symbol: e.target.value })} placeholder="如 binance:BTCUSDT" />
        <label>周期</label>
        <select className="dw-select" value={form.interval} onChange={(e) => setForm({ ...form, interval: e.target.value as Interval })}>
          {INTERVALS.map((i) => <option key={i} value={i}>{i}</option>)}
        </select>
        <label>命名空间</label>
        <input className="dw-input" value={form.namespace} onChange={(e) => setForm({ ...form, namespace: e.target.value })} />
        <label>容忍坏行比</label>
        <input className="dw-input" value={form.max_bad_ratio} onChange={(e) => setForm({ ...form, max_bad_ratio: e.target.value })} placeholder="默认 0.01" />
        <label>CSV 文件</label>
        <input
          className="dw-input"
          type="file"
          accept=".csv,text/csv"
          onChange={(e) => void pickFile(e.target.files?.[0] ?? null)}
        />
      </div>
      <label className="dw-muted">CSV 文本（首行表头：ts,open,high,low,close,volume）</label>
      <textarea
        className="dw-textarea"
        style={{ marginTop: 6 }}
        value={form.text}
        onChange={(e) => setForm({ ...form, text: e.target.value })}
        spellCheck={false}
        placeholder={'ts,open,high,low,close,volume\n1767225600000,100,101,99,100.5,10'}
      />
      <div className="dw-rowflex" style={{ marginTop: 10 }}>
        <button className="dw-btn primary" disabled={busy || !form.text.trim() || !form.symbol.trim()} onClick={doImport}>
          {busy ? '导入中…' : '导入 CSV'}
        </button>
        <button className="dw-btn" disabled={busy || !form.symbol.trim()} onClick={promote}>
          提升到 bronze
        </button>
        <span className="dw-muted">{form.text ? `${form.text.split(/\r?\n/).filter(Boolean).length} 行文本` : '尚未选择文件'}</span>
      </div>
      {err && <div className="dw-find error">{err}</div>}
      {result && (
        <>
          <div className={`dw-find ${result.gate_passed ? 'ok' : 'error'}`}>
            {result.batch_id === 'promote'
              ? `已提升 ${result.written} 行到 bronze`
              : `${result.gate_passed ? '门禁通过' : '门禁未过'} · 可用 ${result.rows_ok}/${result.rows_total} · 写入 ${result.written}`}
          </div>
          {result.findings.map((f, i) => (
            <div key={i} className={`dw-find ${f.severity === 'error' ? 'error' : 'warn'}`}>
              <b>{f.kind}</b> · {f.message}
            </div>
          ))}
          {result.bad_samples.slice(0, 5).map((s, i) => (
            <div key={i} className="dw-find warn">{s}</div>
          ))}
        </>
      )}
    </div>
  );
}

// ───────────────────────────── 页面三：管理 ─────────────────────────────

function ManagePageView({
  wire,
  summaries,
  client,
  now,
  onChanged,
}: {
  wire: DatasetWire[];
  summaries: DatasetSummary[];
  client: DatahubClient;
  now: number;
  onChanged: () => void;
}) {
  const picker = useSymbolPicker(wire);
  const { bars } = useBars(client, picker.dataset, 2, 0);
  const [earliest, setEarliest] = useState<number | null>(null);

  const dataset = picker.dataset;
  const datasetKey = dataset?.dataset_key ?? null;

  useEffect(() => {
    if (!dataset) {
      setEarliest(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const rows = await client.bars({
          symbol: dataset.symbol,
          interval: dataset.interval,
          namespace: dataset.namespace,
          from: 0,
          to: (dataset.as_of ?? Date.now()) + 86_400_000,
          limit: 1,
        });
        if (cancelled) return;
        const ts = rows.length > 0 ? Number(rows[0].ts) : null;
        setEarliest(Number.isFinite(ts as number) ? (ts as number) : null);
      } catch {
        if (!cancelled) setEarliest(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, datasetKey]);

  const summary = summaries.find((s) => s.record.id === picker.dataset?.dataset_key) ?? null;
  const detail = summary ? toDetailProps(summary) : null;

  const last = bars.length > 0 ? bars[bars.length - 1] : null;
  const change = bars.length > 1 && bars[bars.length - 2].close !== 0
    ? (bars[bars.length - 1].close - bars[bars.length - 2].close) / bars[bars.length - 2].close
    : null;
  const tone = change == null || change === 0 ? TONE_COLOR.muted : change > 0 ? UP_COLOR : DOWN_COLOR;

  const rowsForSymbol = useMemo(
    () => summaries.filter((s) => s.record.symbol === picker.symbol),
    [summaries, picker.symbol],
  );

  if (wire.length === 0) {
    return (
      <div className="dw-scroll">
        <EmptyCatalog />
      </div>
    );
  }

  return (
    <div className="dw-scroll">
      <div className="dw-toolbar" style={{ padding: 0, border: 0, marginBottom: 14 }}>
        <select className="dw-select" value={picker.symbol} onChange={(e) => picker.setSymbol(e.target.value)}>
          {picker.symbols.map((s) => (
            <option key={s} value={s}>{s}</option>
          ))}
        </select>
        <div className="dw-pills">
          {picker.intervals.map((iv) => (
            <button
              key={iv}
              className={`dw-pill${iv === picker.interval ? ' active' : ''}`}
              onClick={() => picker.setInterval(iv)}
            >
              {iv}
            </button>
          ))}
        </div>
        <button className="dw-btn" onClick={onChanged}>刷新目录</button>
      </div>

      {dataset && (
        <>
          <div className="dw-kpis">
            <div className="dw-kpi">
              <b>{(Number(dataset.row_count) || 0).toLocaleString()}</b>
              <span>已存 bar 数</span>
            </div>
            <div className="dw-kpi">
              <b className="dw-mono" style={{ fontSize: 14 }}>{fmtTs(earliest)}</b>
              <span>最早一根</span>
            </div>
            <div className="dw-kpi">
              <b className="dw-mono" style={{ fontSize: 14 }}>{fmtTs(dataset.as_of)}</b>
              <span>最新一根</span>
            </div>
            <div className="dw-kpi">
              <b style={{ color: tone }}>{last ? fmtPrice(last.close) : '—'}</b>
              <span>
                最新价
                {change == null ? '' : ` · ${change > 0 ? '+' : ''}${(change * 100).toFixed(2)}%`}
              </span>
            </div>
            <div className="dw-kpi">
              <b style={{ color: detail ? TONE_COLOR[detail.coverageTone] : undefined }}>
                {detail ? detail.coverageText : '—'}
              </b>
              <span>覆盖率（{detail ? `${detail.presentBars}/${detail.expectedBars}` : '—'}）</span>
            </div>
            <div className="dw-kpi">
              <b style={{ color: detail ? TONE_COLOR[detail.freshnessTone] : undefined }}>
                {detail ? detail.freshnessText : '—'}
              </b>
              <span>新鲜度</span>
            </div>
          </div>

          <div className="dw-card">
            <h3>数据集详情（{dataset.symbol} · {dataset.interval}）</h3>
            <div className="dw-kv"><span>数据集键</span><span className="dw-mono">{dataset.dataset_key}</span></div>
            <div className="dw-kv"><span>市场</span><span>{MARKET_LABEL[(dataset.market as keyof typeof MARKET_LABEL) ?? 'CRYPTO'] ?? dataset.market}</span></div>
            <div className="dw-kv"><span>命名空间</span><span className="dw-mono">{dataset.namespace}</span></div>
            <div className="dw-kv"><span>数据源</span><span>{dataset.source}</span></div>
            <div className="dw-kv"><span>更新于</span><span>{fmtTs(dataset.updated_at)}</span></div>
            <div className="dw-kv"><span>契约</span><span style={{ color: detail?.contractValid ? TONE_COLOR.ok : TONE_COLOR.alert }}>{detail?.contractText ?? '—'}</span></div>
            <div className="dw-kv"><span>缺口 bar</span><span style={{ color: detail && detail.missingBars > 0 ? TONE_COLOR.alert : TONE_COLOR.ok }}>{detail ? detail.missingBars : '—'}</span></div>
          </div>

          <div className="dw-card">
            <h3>{picker.symbol} 的全部周期（{rowsForSymbol.length}）</h3>
            <div className="dw-table-wrap">
              <table className="dw-table">
                <thead>
                  <tr>
                    <th>周期</th>
                    <th>命名空间</th>
                    <th className="dw-right">行数</th>
                    <th>最新 bar</th>
                    <th className="dw-right">覆盖率</th>
                    <th>数据源</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {rowsForSymbol.map((s) => {
                    const row = toRowProps(s);
                    const w = wire.find((x) => x.dataset_key === s.record.id);
                    return (
                      <tr key={row.id}>
                        <td>{s.record.interval}</td>
                        <td className="dw-mono">{w?.namespace ?? '—'}</td>
                        <td className="dw-right dw-mono">{(Number(w?.row_count) || 0).toLocaleString()}</td>
                        <td className="dw-mono">{fmtTs(s.record.latestBarTs)}</td>
                        <td className="dw-right" style={{ color: TONE_COLOR[coverageTone(s.coverage.coverageRatio)] }}>
                          {formatPercent(s.coverage.coverageRatio)}
                        </td>
                        <td>{w?.source ?? '—'}</td>
                        <td>
                          <button className="dw-btn" onClick={() => picker.setInterval(s.record.interval)}>选中</button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </div>

          <ClearDataZone dataset={dataset} client={client} now={now} onChanged={onChanged} />
        </>
      )}
    </div>
  );
}

function ClearDataZone({
  dataset,
  client,
  now,
  onChanged,
}: {
  dataset: DatasetWire;
  client: DatahubClient;
  now: number;
  onChanged: () => void;
}) {
  const [range, setRange] = useState<'all' | '7d' | '30d' | 'custom'>('7d');
  const [fromDate, setFromDate] = useState('');
  const [toDate, setToDate] = useState('');
  const [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null);

  const step = intervalMs(dataset.interval as Interval);
  const anchor = dataset.as_of ?? now;

  const bounds = useMemo(() => {
    if (range === 'all') return { from: 0, to: anchor + step };
    if (range === '7d') return { from: anchor - 7 * 86_400_000, to: anchor + step };
    if (range === '30d') return { from: anchor - 30 * 86_400_000, to: anchor + step };
    const from = fromDate ? new Date(`${fromDate}T00:00:00`).getTime() : anchor - 7 * 86_400_000;
    const to = toDate ? new Date(`${toDate}T23:59:59`).getTime() + 1 : anchor + step;
    return { from, to };
  }, [range, anchor, step, fromDate, toDate]);

  async function run() {
    setBusy(true);
    setMsg(null);
    try {
      const r = await client.deleteBars({
        namespace: dataset.namespace,
        symbol: dataset.symbol,
        interval: dataset.interval,
        from: bounds.from,
        to: bounds.to,
      });
      setMsg({ tone: 'ok', text: `已删除 ${r.deleted} 根 bar（${fmtTs(bounds.from)} → ${fmtTs(bounds.to)}）` });
      setConfirm('');
      onChanged();
    } catch (e) {
      setMsg({ tone: 'error', text: errMsg(e) });
    } finally {
      setBusy(false);
    }
  }

  const ready = confirm.trim() === '删除';

  return (
    <div className="dw-danger-zone">
      <h3>清除数据</h3>
      <p className="dw-note" style={{ marginTop: 0, marginBottom: 10 }}>
        只删除当前数据集（{dataset.symbol} · {dataset.interval} · {dataset.namespace}）在所选时间范围内的 bar。
        删除后不可恢复，需要重新同步或导入。
      </p>
      <div className="dw-rowflex">
        {[
          { id: '7d', label: '最近 7 天' },
          { id: '30d', label: '最近 30 天' },
          { id: 'all', label: '全部' },
          { id: 'custom', label: '自定义' },
        ].map((r) => (
          <button
            key={r.id}
            className={`dw-pill${range === r.id ? ' active' : ''}`}
            onClick={() => setRange(r.id as typeof range)}
          >
            {r.label}
          </button>
        ))}
        {range === 'custom' && (
          <>
            <input className="dw-input" style={{ width: 160 }} type="date" value={fromDate} onChange={(e) => setFromDate(e.target.value)} />
            <span className="dw-muted">→</span>
            <input className="dw-input" style={{ width: 160 }} type="date" value={toDate} onChange={(e) => setToDate(e.target.value)} />
          </>
        )}
      </div>
      <div className="dw-rowflex">
        <span className="dw-muted">
          将删除范围：{fmtTs(range === 'all' ? null : bounds.from)} → {fmtTs(bounds.to)}
        </span>
        <input
          className="dw-input"
          style={{ width: 140 }}
          placeholder="输入“删除”确认"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        <button className="dw-btn danger" disabled={busy || !ready} onClick={run}>
          {busy ? '删除中…' : '执行删除'}
        </button>
      </div>
      {msg && <div className={`dw-find ${msg.tone === 'ok' ? 'ok' : 'error'}`}>{msg.text}</div>}
    </div>
  );
}

function EmptyCatalog() {
  return (
    <p className="dw-empty">
      目录为空。先在「数据」页用 CSV 导入行情，或让 Agent 创建同步任务：
      <br />
      <code>POST /api/datasets</code> → <code>POST /api/jobs</code> →{' '}
      <code>POST /api/jobs/&lt;id&gt;/run</code>
    </p>
  );
}

// ───────────────────────────── 格式工具 ─────────────────────────────

function fmtTs(ts: number | null | undefined): string {
  if (ts == null || !Number.isFinite(ts) || ts <= 0) return '—';
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fmtPrice(v: number): string {
  if (!Number.isFinite(v)) return '—';
  const digits = v >= 1000 ? 2 : v >= 1 ? 4 : 6;
  return v.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: digits });
}

function fmtCell(v: unknown): string {
  if (v === null || v === undefined) return '∅';
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
