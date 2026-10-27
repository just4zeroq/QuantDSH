import React, { useCallback, useEffect, useMemo, useState } from 'react';
import type { CoverageCell, DatasetSummary, Interval } from '../domain';
import { intervalMs, INTERVALS, summarizeDataset } from '../domain';
import { toDetailProps, toRowProps } from './dataset-presentation';
import { useDatahubWorkbench, type DatahubClient } from './use-datahub';
import { CoverageHeatmap } from './CoverageHeatmap';
import type {
  DatasetWire,
  ImportRecordWire,
  QualityReportWire,
  SourceWire,
  SyncJobWire,
} from '../datahub/client';

// UI 只消费 domain 层的纯逻辑（覆盖率 / 新鲜度 / 契约口径的唯一实现），
// 与 Agent 工具、回测加载器共用同一套判定 —— 避免"页面看到的和引擎认为的不一样"。
//
// 版式契约（产品决定，勿随意改回）：
// 侧栏只有一个「数据工坊」入口，进入后按**工作流阶段**分两个页签：
//   数据接入 = 把数据弄进来：数据源（库里的行）→ 接入任务 → 手动导入 → 批次审计
//   数据资产 = 手里已有什么：台账 → 体检 → 覆盖率 → 只读 SQL
// K 线不再属于这里 —— 它是「行情」工作台（quantdsh-plugin-market）的职责。
//
// 数据来源：本地行情数据服务（services/datahub-api）通过 HTTP 提供。
// 页面是众多只读消费者之一；服务是唯一写者（DuckDB 跨进程只允许一个写者）。

const TONE_COLOR: Record<string, string> = {
  ok: 'var(--dsw-alias-status-success, #2ba471)',
  warn: 'var(--dsw-alias-status-warning, #e37318)',
  alert: 'var(--dsw-alias-status-error, #d54941)',
  muted: 'var(--dsw-alias-label-tertiary, #959da6)',
};

type TabId = 'ingest' | 'assets';

const TABS: { id: TabId; label: string; hint: string }[] = [
  { id: 'ingest', label: '数据接入', hint: '来源 · 任务 · 批次审计' },
  { id: 'assets', label: '数据资产', hint: '台账 · 体检 · 覆盖率' },
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
.dw-btn{font:inherit;font-size:12px;padding:5px 12px;border-radius:7px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129);cursor:pointer;white-space:nowrap}
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
.dw-table tbody tr.sel{background:var(--dsw-alias-bg-layer-2,#e8f0ff)}
.dw-table tbody tr.clickable{cursor:pointer}
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
.dw-muted{color:var(--dsw-alias-label-tertiary,#959da6);font-size:12px}
.dw-danger-zone{border:1px solid rgba(213,73,65,.35);border-radius:10px;padding:14px 16px;margin-bottom:14px;background:rgba(213,73,65,.03)}
.dw-danger-zone h3{color:var(--dsw-alias-status-error,#d54941)}
.dw-sec{display:flex;align-items:baseline;gap:8px;margin:0 0 10px}
.dw-sec h3{margin:0;font-size:13px;font-weight:600}
.dw-sec span{font-size:12px;color:var(--dsw-alias-label-tertiary,#959da6)}
.dw-cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(248px,1fr));gap:12px}
.dw-src{border:1px solid var(--dsw-alias-border-l2,#dce2e9);border-radius:10px;padding:12px 14px;display:flex;flex-direction:column;gap:8px;min-width:0}
.dw-src.off{opacity:.68}
.dw-src-h{display:flex;justify-content:space-between;gap:8px;align-items:baseline}
.dw-src-h b{font-size:13px;font-weight:600}
.dw-src-meta{font-size:11.5px;color:var(--dsw-alias-label-secondary,#657185);line-height:1.65;word-break:break-all}
.dw-src-acts{display:flex;gap:6px;flex-wrap:wrap;margin-top:auto}
.dw-badge{font-size:11px;padding:2px 8px;border-radius:999px;border:1px solid currentColor;white-space:nowrap;line-height:1.4}
.dw-badge.ok{color:var(--dsw-alias-status-success,#2ba471)}
.dw-badge.warn{color:var(--dsw-alias-status-warning,#e37318)}
.dw-badge.err{color:var(--dsw-alias-status-error,#d54941)}
.dw-badge.off{color:var(--dsw-alias-label-tertiary,#959da6)}
.dw-split{display:grid;grid-template-columns:minmax(0,1.55fr) minmax(0,1fr);gap:14px;align-items:start}
@media (max-width:1240px){.dw-split{grid-template-columns:minmax(0,1fr)}}
.dw-acts{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-left:auto}
`;

export function DataWorkbench({ nowTs }: { nowTs: number }) {
  // 页面自己维持时间基准：面板只挂载一次，用传入值会让"新鲜度"永久停在上次进入时刻。
  const [now, setNow] = useState(nowTs);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const state = useDatahubWorkbench(now);
  const [tab, setTab] = useState<TabId>('ingest');
  // 写操作（建任务、探测、导入）之后，各区块都要重读：一个自增数就是最省的信号。
  const [nonce, setNonce] = useState(0);
  const reload = state.kind === 'ready' ? state.reload : () => undefined;
  const bump = useCallback(() => {
    setNonce((n) => n + 1);
    reload();
  }, [reload]);

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
        ) : tab === 'ingest' ? (
          <IngestPage client={state.client} wire={wire} nonce={nonce} onChanged={bump} />
        ) : (
          <AssetsPage
            client={state.client}
            wire={wire}
            summaries={summaries}
            now={now}
            nonce={nonce}
            onChanged={bump}
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
          {state.message}。数据服务随应用自动启动（安装包自带 Python 运行时）。
          <br />
          若一直连不上，请看 <code>%LOCALAPPDATA%\QuantDSH\data\datahub.log</code> 的报错，
          然后完全退出并重新打开 QuantDSH。
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

// ───────────────────────────── 页签一：数据接入 ─────────────────────────────

function IngestPage({
  client,
  wire,
  nonce,
  onChanged,
}: {
  client: DatahubClient;
  wire: DatasetWire[];
  nonce: number;
  onChanged: () => void;
}) {
  return (
    <div className="dw-scroll">
      <SourcesSection client={client} nonce={nonce} onChanged={onChanged} />
      <JobsSection client={client} nonce={nonce} onChanged={onChanged} />
      <ImportSection wire={wire} client={client} onChanged={onChanged} />
      <BatchesSection client={client} nonce={nonce} />
    </div>
  );
}

/** 数据源 = `sources` 表里的一行。新增一家交易所是加一行配置，不是发一次版。 */
function SourcesSection({
  client,
  nonce,
  onChanged,
}: {
  client: DatahubClient;
  nonce: number;
  onChanged: () => void;
}) {
  const [sources, setSources] = useState<SourceWire[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [probe, setProbe] = useState<{ id: string; ok: boolean; text: string } | null>(null);
  const [editing, setEditing] = useState<SourceWire | null>(null);
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    try {
      const payload = await client.sourceList();
      setSources(payload.all);
      setErr(null);
    } catch (e) {
      setErr(errMsg(e));
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load, nonce]);

  async function runProbe(id: string) {
    setBusy(id);
    setProbe(null);
    try {
      const result = await client.probeSource(id);
      const caps = Object.entries(result.capabilities ?? {})
        .map(([k, v]) => `${k}:${(v as { supported?: boolean })?.supported === false ? '不支持' : 'ok'}`)
        .join(' · ');
      setProbe({
        id,
        ok: result.status === 'ok',
        text: `${result.status === 'ok' ? '可用' : '不可达'}${caps ? ` · ${caps}` : ''}${
          result.notes?.length ? ` · ${result.notes.slice(0, 2).join('；')}` : ''
        }`,
      });
      await load();
      onChanged();
    } catch (e) {
      setProbe({ id, ok: false, text: errMsg(e) });
    } finally {
      setBusy(null);
    }
  }

  async function toggle(source: SourceWire) {
    setBusy(source.id);
    try {
      await client.upsertSource({ id: source.id, enabled: !source.enabled });
      await load();
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(null);
    }
  }

  async function remove(source: SourceWire) {
    setBusy(source.id);
    try {
      await client.deleteSource(source.id);
      await load();
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(null);
    }
  }

  const enabled = (sources ?? []).filter((s) => s.enabled).length;
  const ready = (sources ?? []).filter((s) => s.status === 'ok').length;

  return (
    <div className="dw-card">
      <div className="dw-sec">
        <h3>数据源</h3>
        <span>
          {sources == null
            ? '装载中…'
            : `${sources.length} 个 · ${enabled} 个启用 · ${ready} 个已探测可用`}
        </span>
        <div className="dw-acts">
          <button className="dw-btn" onClick={() => void load()} disabled={busy !== null}>
            刷新
          </button>
          <button
            className="dw-btn primary"
            onClick={() => {
              setCreating(true);
              setEditing(null);
            }}
          >
            + 新增数据源
          </button>
        </div>
      </div>

      {err && <div className="dw-find error">{err}</div>}

      {(creating || editing) && (
        <SourceEditor
          initial={editing}
          onCancel={() => {
            setCreating(false);
            setEditing(null);
          }}
          onSaved={async () => {
            setCreating(false);
            setEditing(null);
            await load();
            onChanged();
          }}
          client={client}
        />
      )}

      {sources == null ? (
        <p className="dw-muted">正在读取数据源配置…</p>
      ) : (
        <div className="dw-cards">
          {sources.map((s) => {
            const tone =
              !s.enabled ? 'off' : s.status === 'ok' ? 'ok' : s.status === 'unreachable' ? 'err' : 'warn';
            const label =
              !s.enabled
                ? '已停用'
                : s.status === 'ok'
                  ? '可用'
                  : s.status === 'unreachable'
                    ? '不可达'
                    : '未探测';
            return (
              <div key={s.id} className={`dw-src${s.enabled ? '' : ' off'}`}>
                <div className="dw-src-h">
                  <b>{s.label ?? s.id}</b>
                  <span className={`dw-badge ${tone}`}>{label}</span>
                </div>
                <div className="dw-src-meta">
                  {s.provider}
                  {s.venue ? ` · ${s.venue}` : ''} · {s.kind ?? 'exchange'}
                  <br />
                  品种：{(s.instTypes ?? []).join(' / ') || '—'}
                  {s.baseUrls?.length ? (
                    <>
                      <br />
                      {s.baseUrls[0]}
                    </>
                  ) : null}
                  {s.capabilities ? (
                    <>
                      <br />
                      已探测能力：{Object.keys(s.capabilities).join(' / ')}
                    </>
                  ) : null}
                </div>
                {probe?.id === s.id && (
                  <div className={`dw-find ${probe.ok ? 'ok' : 'error'}`} style={{ margin: 0 }}>
                    {probe.text}
                  </div>
                )}
                <div className="dw-src-acts">
                  <button
                    className="dw-btn"
                    disabled={busy === s.id || !s.enabled}
                    onClick={() => void runProbe(s.id)}
                    title={s.enabled ? '真实探测一次，结果写回能力列' : '先启用再探测'}
                  >
                    {busy === s.id ? '探测中…' : '自检'}
                  </button>
                  <button className="dw-btn" disabled={busy === s.id} onClick={() => void toggle(s)}>
                    {s.enabled ? '停用' : '启用'}
                  </button>
                  <button
                    className="dw-btn"
                    disabled={busy === s.id}
                    onClick={() => {
                      setEditing(s);
                      setCreating(false);
                    }}
                  >
                    编辑
                  </button>
                  <button className="dw-btn danger" disabled={busy === s.id} onClick={() => void remove(s)}>
                    删除
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <p className="dw-note">
        「已探测能力」是**探测结果的缓存**，不是声明：只有自检成功过的源才会写这一列，
        界面只信这一列。新增交易所 = 加一行 <code>provider=ccxt, venue=&lt;ccxt id&gt;</code> 的配置，
        不需要发版。
      </p>
    </div>
  );
}

function SourceEditor({
  client,
  initial,
  onCancel,
  onSaved,
}: {
  client: DatahubClient;
  initial: SourceWire | null;
  onCancel: () => void;
  onSaved: () => void;
}) {
  const [form, setForm] = useState({
    id: initial?.id ?? '',
    label: initial?.label ?? '',
    provider: initial?.provider ?? 'ccxt',
    venue: initial?.venue ?? '',
    kind: initial?.kind ?? 'exchange',
    baseUrls: (initial?.baseUrls ?? []).join('\n'),
    instTypes: (initial?.instTypes ?? ['spot']).join(','),
    priority: String(initial?.priority ?? 100),
    enabled: initial?.enabled ?? true,
  });
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    setErr(null);
    try {
      await client.upsertSource({
        id: form.id.trim(),
        label: form.label.trim() || form.id.trim(),
        provider: form.provider.trim(),
        venue: form.venue.trim() || undefined,
        kind: form.kind.trim() || 'exchange',
        base_urls: form.baseUrls
          .split('\n')
          .map((s) => s.trim())
          .filter(Boolean),
        inst_types: form.instTypes
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean),
        priority: Number(form.priority) || 100,
        enabled: form.enabled,
      });
      onSaved();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dw-card" style={{ background: 'var(--dsw-alias-bg-layer-1,#f2f3f5)' }}>
      <h3>{initial ? `编辑数据源：${initial.id}` : '新增数据源'}</h3>
      <div className="dw-form">
        <label>ID</label>
        <input
          className="dw-input"
          value={form.id}
          disabled={Boolean(initial)}
          onChange={(e) => setForm({ ...form, id: e.target.value })}
          placeholder="binance / okx / official / 自定义"
        />
        <label>名称</label>
        <input
          className="dw-input"
          value={form.label}
          onChange={(e) => setForm({ ...form, label: e.target.value })}
          placeholder="显示用名称"
        />
        <label>provider</label>
        <select
          className="dw-select"
          value={form.provider}
          onChange={(e) => setForm({ ...form, provider: e.target.value })}
        >
          <option value="ccxt">ccxt（交易所直连）</option>
          <option value="official">official（我们的聚合 API）</option>
          <option value="replay">replay（离线造数，回归用）</option>
        </select>
        <label>venue</label>
        <input
          className="dw-input"
          value={form.venue}
          onChange={(e) => setForm({ ...form, venue: e.target.value })}
          placeholder="ccxt 的交易所 id，如 binance；聚合源留空"
        />
        <label>品种</label>
        <input
          className="dw-input"
          value={form.instTypes}
          onChange={(e) => setForm({ ...form, instTypes: e.target.value })}
          placeholder="spot,swap"
        />
        <label>base_urls</label>
        <textarea
          className="dw-textarea"
          style={{ minHeight: 64 }}
          value={form.baseUrls}
          onChange={(e) => setForm({ ...form, baseUrls: e.target.value })}
          placeholder="每行一个候选，按顺序回退（可留空用 ccxt 默认）"
          spellCheck={false}
        />
        <label>优先级</label>
        <input
          className="dw-input"
          type="number"
          value={form.priority}
          onChange={(e) => setForm({ ...form, priority: e.target.value })}
        />
        <label>启用</label>
        <label style={{ textAlign: 'left' }}>
          <input
            type="checkbox"
            checked={form.enabled}
            onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
          />{' '}
          启用后才会出现在建任务的来源下拉里
        </label>
      </div>
      {err && <div className="dw-find error">{err}</div>}
      <div className="dw-rowflex" style={{ marginBottom: 0 }}>
        <button className="dw-btn primary" disabled={busy || !form.id.trim()} onClick={() => void save()}>
          {busy ? '保存中…' : '保存'}
        </button>
        <button className="dw-btn" onClick={onCancel} disabled={busy}>
          取消
        </button>
      </div>
    </div>
  );
}

/** 接入任务 = 「从某来源、按某周期、持续把某些标的的 bar 拉进库」。 */
function JobsSection({
  client,
  nonce,
  onChanged,
}: {
  client: DatahubClient;
  nonce: number;
  onChanged: () => void;
}) {
  const [jobs, setJobs] = useState<SyncJobWire[] | null>(null);
  const [sources, setSources] = useState<SourceWire[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  const load = useCallback(async () => {
    try {
      const [list, src] = await Promise.all([client.jobs(), client.sourceList()]);
      setJobs(list);
      setSources(src.items.filter((s) => s.enabled));
      setErr(null);
    } catch (e) {
      setErr(errMsg(e));
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load, nonce]);

  async function run(job: SyncJobWire) {
    setBusy(job.id);
    setErr(null);
    try {
      await client.runJob(job.id, {});
      await load();
      onChanged();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(null);
    }
  }

  async function toggle(job: SyncJobWire) {
    setBusy(job.id);
    try {
      await client.updateJob(job.id, { enabled: !job.enabled });
      await load();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(null);
    }
  }

  async function remove(job: SyncJobWire) {
    setBusy(job.id);
    try {
      await client.deleteJob(job.id);
      await load();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(null);
    }
  }

  const running = (jobs ?? []).filter((j) => j.status === 'running').length;

  return (
    <div className="dw-card">
      <div className="dw-sec">
        <h3>接入任务</h3>
        <span>
          {jobs == null ? '装载中…' : `${jobs.length} 个${running ? ` · ${running} 个运行中` : ''}`}
        </span>
        <div className="dw-acts">
          <button className="dw-btn" onClick={() => void load()} disabled={busy !== null}>
            刷新
          </button>
          <button className="dw-btn primary" onClick={() => setOpen((v) => !v)}>
            {open ? '收起' : '+ 新建接入任务'}
          </button>
        </div>
      </div>

      {err && <div className="dw-find error">{err}</div>}

      {open && (
        <JobEditor
          client={client}
          sources={sources}
          onCancel={() => setOpen(false)}
          onCreated={async () => {
            setOpen(false);
            await load();
            onChanged();
          }}
        />
      )}

      <div className="dw-table-wrap" style={{ maxHeight: 280 }}>
        <table className="dw-table">
          <thead>
            <tr>
              <th>状态</th>
              <th>任务</th>
              <th>来源</th>
              <th className="dw-right">已写入</th>
              <th>周期</th>
              <th>上次</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {jobs == null ? (
              <tr>
                <td colSpan={7} className="dw-empty-cell">装载中…</td>
              </tr>
            ) : jobs.length === 0 ? (
              <tr>
                <td colSpan={7} className="dw-empty-cell">
                  还没有接入任务。先在「数据源」里确认来源可用，再新建一个任务。
                </td>
              </tr>
            ) : (
              jobs.map((j) => (
                <tr key={j.id}>
                  <td>
                    <span
                      className={`dw-badge ${
                        j.status === 'error' ? 'err' : j.status === 'running' ? 'warn' : j.enabled ? 'ok' : 'off'
                      }`}
                    >
                      {j.enabled ? j.status || 'idle' : '已暂停'}
                    </span>
                  </td>
                  <td className="dw-mono">
                    {(j.symbols ?? []).join(', ') || '—'} · {j.interval}
                    {j.mode ? ` · ${j.mode}` : ''}
                  </td>
                  <td>{j.source}</td>
                  <td className="dw-right dw-mono">{(j.rows_written ?? 0).toLocaleString()}</td>
                  <td className="dw-mono">{j.cron || '—'}</td>
                  <td className="dw-mono">
                    {fmtTs(j.last_run_at)}
                    {j.last_error ? (
                      <div className="dw-muted" style={{ color: TONE_COLOR.alert }}>
                        {j.last_error.slice(0, 60)}
                      </div>
                    ) : null}
                  </td>
                  <td>
                    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                      <button className="dw-btn" disabled={busy === j.id} onClick={() => void run(j)}>
                        {busy === j.id ? '运行中…' : '立即运行'}
                      </button>
                      <button className="dw-btn" disabled={busy === j.id} onClick={() => void toggle(j)}>
                        {j.enabled ? '暂停' : '恢复'}
                      </button>
                      <button className="dw-btn danger" disabled={busy === j.id} onClick={() => void remove(j)}>
                        删除
                      </button>
                    </div>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function JobEditor({
  client,
  sources,
  onCancel,
  onCreated,
}: {
  client: DatahubClient;
  sources: SourceWire[];
  onCancel: () => void;
  onCreated: () => void;
}) {
  const [sourceId, setSourceId] = useState(sources[0]?.id ?? '');
  const source = sources.find((s) => s.id === sourceId) ?? null;
  const [symbols, setSymbols] = useState('BTC/USDT');
  const [intervalValue, setIntervalValue] = useState<Interval>('1m');
  const [instType, setInstType] = useState(source?.instTypes?.[0] ?? 'spot');
  const [mode, setMode] = useState('incremental');
  const [cron, setCron] = useState('');
  const [fromDate, setFromDate] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const next = sources.find((s) => s.id === sourceId);
    const options = next?.instTypes?.length ? next.instTypes : ['spot'];
    if (!options.includes(instType)) setInstType(options[0]);
  }, [sourceId, sources, instType]);

  async function create() {
    setBusy(true);
    setErr(null);
    try {
      const list = symbols
        .split(/[,，\s]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      await client.createJob({
        source: sourceId,
        symbols: list,
        interval: intervalValue,
        inst_type: instType,
        mode,
        cron: cron.trim() || null,
        from_ts: fromDate ? new Date(`${fromDate}T00:00:00`).getTime() : null,
        enabled: true,
      });
      onCreated();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dw-card" style={{ background: 'var(--dsw-alias-bg-layer-1,#f2f3f5)' }}>
      <h3>新建接入任务</h3>
      <div className="dw-form">
        <label>数据源</label>
        <select className="dw-select" value={sourceId} onChange={(e) => setSourceId(e.target.value)}>
          {sources.length === 0 ? (
            <option value="">（没有启用的数据源，先去上面启用一个）</option>
          ) : (
            sources.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label ?? s.id}（{s.provider}
                {s.venue ? ` · ${s.venue}` : ''}）
              </option>
            ))
          )}
        </select>
        <label>标的</label>
        <input
          className="dw-input"
          value={symbols}
          onChange={(e) => setSymbols(e.target.value)}
          placeholder="多个用逗号分隔，如 BTC/USDT, ETH/USDT"
        />
        <label>周期</label>
        <select
          className="dw-select"
          value={intervalValue}
          onChange={(e) => setIntervalValue(e.target.value as Interval)}
        >
          {INTERVALS.map((iv) => (
            <option key={iv} value={iv}>
              {iv}
            </option>
          ))}
        </select>
        <label>品种类型</label>
        <select className="dw-select" value={instType} onChange={(e) => setInstType(e.target.value)}>
          {(source?.instTypes?.length ? source.instTypes : ['spot']).map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
        <label>模式</label>
        <select className="dw-select" value={mode} onChange={(e) => setMode(e.target.value)}>
          <option value="incremental">incremental（只补最新）</option>
          <option value="backfill">backfill（按起始日整段回补）</option>
        </select>
        <label>起始日</label>
        <input
          className="dw-input"
          type="date"
          value={fromDate}
          onChange={(e) => setFromDate(e.target.value)}
        />
        <label>定时表达式</label>
        <input
          className="dw-input"
          value={cron}
          onChange={(e) => setCron(e.target.value)}
          placeholder="可留空；留空即只手动运行"
        />
      </div>
      <p className="dw-note" style={{ marginTop: 0 }}>
        任务键由服务端按 <code>venue:标的:品种:ohlcv:周期</code> 生成，不需要手填。
      </p>
      {err && <div className="dw-find error">{err}</div>}
      <div className="dw-rowflex" style={{ marginBottom: 0 }}>
        <button
          className="dw-btn primary"
          disabled={busy || !sourceId || !symbols.trim()}
          onClick={() => void create()}
        >
          {busy ? '创建中…' : '创建任务'}
        </button>
        <button className="dw-btn" onClick={onCancel} disabled={busy}>
          取消
        </button>
      </div>
    </div>
  );
}

/** 批次审计：每批写入都留痕（来源、写入行数、被拒行数、门禁结论）。 */
function BatchesSection({ client, nonce }: { client: DatahubClient; nonce: number }) {
  const [rows, setRows] = useState<ImportRecordWire[] | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await client.listImports();
        if (!cancelled) {
          setRows(list.slice(0, 20));
          setErr(null);
        }
      } catch (e) {
        if (!cancelled) setErr(errMsg(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, nonce]);

  return (
    <div className="dw-card">
      <div className="dw-sec">
        <h3>批次审计</h3>
        <span>{rows == null ? '装载中…' : `最近 ${rows.length} 批`}</span>
      </div>
      {err && <div className="dw-find error">{err}</div>}
      <div className="dw-table-wrap" style={{ maxHeight: 260 }}>
        <table className="dw-table">
          <thead>
            <tr>
              <th>时间</th>
              <th>数据集</th>
              <th>文件</th>
              <th className="dw-right">总行</th>
              <th className="dw-right">可用</th>
              <th className="dw-right">被拒</th>
              <th>门禁</th>
            </tr>
          </thead>
          <tbody>
            {rows == null ? (
              <tr>
                <td colSpan={7} className="dw-empty-cell">装载中…</td>
              </tr>
            ) : rows.length === 0 ? (
              <tr>
                <td colSpan={7} className="dw-empty-cell">
                  还没有批次。CSV 导入或同步任务执行后，这里会出现审计记录。
                </td>
              </tr>
            ) : (
              rows.map((r) => (
                <tr key={r.id}>
                  <td className="dw-mono">{fmtTs(r.created_at)}</td>
                  <td className="dw-mono">{r.dataset_key}</td>
                  <td>{r.file}</td>
                  <td className="dw-right dw-mono">{r.rows_total}</td>
                  <td className="dw-right dw-mono">{r.rows_ok}</td>
                  <td className="dw-right dw-mono">{r.rows_bad}</td>
                  <td>
                    <span className={`dw-badge ${r.gate_passed ? 'ok' : 'err'}`}>
                      {r.gate_passed ? '通过' : '未过'}
                    </span>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
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
      <div className="dw-sec">
        <h3>手动导入</h3>
        <span>CSV 先进 imported 命名空间，通过门禁后可提升 bronze</span>
      </div>
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
        <input className="dw-input" value={form.dataset_key} onChange={(e) => setForm({ ...form, dataset_key: e.target.value })} placeholder="如 binance:BTC/USDT:spot:ohlcv:1m" />
        <label>标的</label>
        <input className="dw-input" value={form.symbol} onChange={(e) => setForm({ ...form, symbol: e.target.value })} placeholder="如 BTC/USDT" />
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

// ───────────────────────────── 页签二：数据资产 ─────────────────────────────

function AssetsPage({
  client,
  wire,
  summaries,
  now,
  nonce,
  onChanged,
}: {
  client: DatahubClient;
  wire: DatasetWire[];
  summaries: DatasetSummary[];
  now: number;
  nonce: number;
  onChanged: () => void;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const [onlyIssues, setOnlyIssues] = useState(false);
  const [sweep, setSweep] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    if (summaries.length === 0) {
      setSelected(null);
      return;
    }
    if (!selected || !summaries.some((s) => s.record.id === selected)) {
      setSelected(summaries[0].record.id);
    }
  }, [summaries, selected]);

  const rows = useMemo(
    () => summaries.filter((s) => (onlyIssues ? toRowProps(s).coverageTone !== 'ok' : true)),
    [summaries, onlyIssues],
  );

  const totalRows = useMemo(
    () => wire.reduce((sum, w) => sum + (Number(w.row_count) || 0), 0),
    [wire],
  );
  const latest = useMemo(() => {
    let best: number | null = null;
    for (const s of summaries) {
      const ts = s.record.latestBarTs ?? null;
      if (ts != null && (best == null || ts > best)) best = ts;
    }
    return best;
  }, [summaries]);
  const issues = summaries.filter((s) => toRowProps(s).coverageTone !== 'ok').length;
  const symbols = useMemo(() => new Set(summaries.map((s) => s.record.symbol)).size, [summaries]);

  const selectedSummary = summaries.find((s) => s.record.id === selected) ?? null;
  const selectedWire = wire.find((w) => w.dataset_key === selected) ?? null;

  async function runSweep() {
    const targets = summaries.slice(0, 50);
    setSweep({ done: 0, total: targets.length });
    for (const [index, s] of targets.entries()) {
      try {
        await client.quality({
          symbol: s.record.symbol,
          interval: s.record.interval,
          from_ts: 0,
          to_ts: now,
        });
      } catch {
        /* 单个体检失败不该中断整轮巡检 */
      }
      setSweep({ done: index + 1, total: targets.length });
    }
    setSweep(null);
    onChanged();
  }

  function exportReport() {
    const payload = {
      generated_at: new Date(now).toISOString(),
      datasets: summaries.map((s) => {
        const row = toRowProps(s);
        const detail = toDetailProps(s);
        return {
          dataset_key: row.id,
          symbol: row.symbol,
          interval: row.interval,
          coverage: detail.coverageText,
          present_bars: detail.presentBars,
          expected_bars: detail.expectedBars,
          missing_bars: detail.missingBars,
          freshness: detail.freshnessText,
          contract: detail.contractText,
        };
      }),
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `quantdsh-data-assets-${new Date(now).toISOString().slice(0, 10)}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
  }

  return (
    <div className="dw-scroll">
      <div className="dw-sec">
        <h3 style={{ fontSize: 14 }}>数据资产</h3>
        <span>台账（存在性）→ 体检（可用性）→ 覆盖率（完整性）</span>
        <div className="dw-acts">
          <button className="dw-btn" onClick={() => void runSweep()} disabled={sweep !== null}>
            {sweep ? `巡检中 ${sweep.done}/${sweep.total}` : '全库巡检'}
          </button>
          <button className="dw-btn" onClick={exportReport} disabled={summaries.length === 0}>
            导出报告
          </button>
        </div>
      </div>

      <div className="dw-kpis">
        <div className="dw-kpi">
          <b>{summaries.length}</b>
          <span>数据集 · {symbols} 个标的</span>
        </div>
        <div className="dw-kpi">
          <b>{totalRows.toLocaleString()}</b>
          <span>总 bar 数</span>
        </div>
        <div className="dw-kpi">
          <b className="dw-mono" style={{ fontSize: 15 }}>{fmtTs(latest)}</b>
          <span>最新一根</span>
        </div>
        <div className="dw-kpi">
          <b style={{ color: issues > 0 ? TONE_COLOR.warn : TONE_COLOR.ok }}>{issues}</b>
          <span>覆盖不足的数据集</span>
        </div>
      </div>

      {summaries.length === 0 ? (
        <EmptyCatalog />
      ) : (
        <div className="dw-split">
          <div className="dw-card" style={{ minWidth: 0 }}>
            <div className="dw-sec">
              <h3>资产台账</h3>
              <span>{rows.length} 个数据集</span>
              <div className="dw-acts">
                <button
                  className={`dw-btn${onlyIssues ? ' primary' : ''}`}
                  onClick={() => setOnlyIssues((v) => !v)}
                >
                  {onlyIssues ? '显示全部' : '仅异常'}
                </button>
              </div>
            </div>
            <div className="dw-table-wrap" style={{ maxHeight: 460 }}>
              <table className="dw-table">
                <thead>
                  <tr>
                    <th>数据集</th>
                    <th>来源</th>
                    <th className="dw-right">行数</th>
                    <th className="dw-right">覆盖</th>
                    <th>最新 bar</th>
                    <th>体检</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.length === 0 ? (
                    <tr>
                      <td colSpan={6} className="dw-empty-cell">没有满足条件的数据集</td>
                    </tr>
                  ) : (
                    rows.map((s) => {
                      const row = toRowProps(s);
                      const w = wire.find((x) => x.dataset_key === s.record.id);
                      return (
                        <tr
                          key={row.id}
                          className={`clickable${row.id === selected ? ' sel' : ''}`}
                          onClick={() => setSelected(row.id)}
                        >
                          <td className="dw-mono">
                            {row.symbol} · {row.interval}
                          </td>
                          <td>{w?.source ?? '—'}</td>
                          <td className="dw-right dw-mono">{(Number(w?.row_count) || 0).toLocaleString()}</td>
                          <td className="dw-right" style={{ color: TONE_COLOR[row.coverageTone] }}>
                            {row.coverageText}
                          </td>
                          <td className="dw-mono">{fmtTs(s.record.latestBarTs)}</td>
                          <td>
                            <span
                              className={`dw-badge ${
                                row.coverageTone === 'ok' ? 'ok' : row.coverageTone === 'warn' ? 'warn' : 'err'
                              }`}
                            >
                              {row.contractValid ? row.freshnessText : '契约缺口'}
                            </span>
                          </td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
            <p className="dw-note">点一行看右侧详情；体检按「存在性 → 完整性 → 可用性」三段看。</p>
          </div>

          <div style={{ minWidth: 0 }}>
            {selectedSummary && selectedWire ? (
              <DatasetDetailPanel
                key={selectedSummary.record.id}
                client={client}
                summary={selectedSummary}
                dataset={selectedWire}
                now={now}
                nonce={nonce}
                onChanged={onChanged}
              />
            ) : (
              <div className="dw-card">
                <h3>数据集详情</h3>
                <p className="dw-muted">左侧选择一个数据集。</p>
              </div>
            )}
          </div>
        </div>
      )}

      <SqlSection client={client} onChanged={onChanged} />
    </div>
  );
}

function DatasetDetailPanel({
  client,
  summary,
  dataset,
  now,
  nonce,
  onChanged,
}: {
  client: DatahubClient;
  summary: DatasetSummary;
  dataset: DatasetWire;
  now: number;
  nonce: number;
  onChanged: () => void;
}) {
  const row = toRowProps(summary);
  const detail = toDetailProps(summary);

  const [quality, setQuality] = useState<QualityReportWire | null>(null);
  const [qualityBusy, setQualityBusy] = useState(false);
  const [cells, setCells] = useState<CoverageCell[]>([]);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [report, coverage] = await Promise.all([
          client.latestQuality(dataset.dataset_key).catch(() => null),
          client.coverage(dataset.dataset_key).catch(() => ({ datasetKey: dataset.dataset_key, cells: [] })),
        ]);
        if (cancelled) return;
        setQuality(report);
        setCells((coverage.cells ?? []) as CoverageCell[]);
      } catch (e) {
        if (!cancelled) setErr(errMsg(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, dataset.dataset_key, nonce]);

  async function rerun() {
    setQualityBusy(true);
    setErr(null);
    try {
      const report = await client.quality({
        symbol: dataset.symbol,
        interval: dataset.interval,
        namespace: dataset.namespace,
        from_ts: 0,
        to_ts: now,
      });
      setQuality(report);
      const coverage = await client
        .coverage(dataset.dataset_key)
        .catch(() => ({ datasetKey: dataset.dataset_key, cells: [] }));
      setCells((coverage.cells ?? []) as CoverageCell[]);
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setQualityBusy(false);
    }
  }

  return (
    <>
      <div className="dw-card">
        <div className="dw-sec">
          <h3>
            {dataset.symbol} · {dataset.interval}
          </h3>
          <div className="dw-acts">
            <button className="dw-btn" disabled={qualityBusy} onClick={() => void rerun()}>
              {qualityBusy ? '体检中…' : '重新体检'}
            </button>
          </div>
        </div>
        {err && <div className="dw-find error">{err}</div>}
        <div className="dw-kv">
          <span>数据集键</span>
          <span className="dw-mono">{dataset.dataset_key}</span>
        </div>
        <div className="dw-kv">
          <span>来源</span>
          <span>{dataset.source}</span>
        </div>
        <div className="dw-kv">
          <span>命名空间</span>
          <span className="dw-mono">{dataset.namespace}</span>
        </div>
        <div className="dw-kv">
          <span>覆盖率</span>
          <span style={{ color: TONE_COLOR[detail.coverageTone] }}>
            {detail.coverageText}（{detail.presentBars}/{detail.expectedBars}）
          </span>
        </div>
        <div className="dw-kv">
          <span>缺口 bar</span>
          <span style={{ color: detail.missingBars > 0 ? TONE_COLOR.alert : TONE_COLOR.ok }}>
            {detail.missingBars}
          </span>
        </div>
        <div className="dw-kv">
          <span>新鲜度</span>
          <span style={{ color: TONE_COLOR[detail.freshnessTone] }}>{detail.freshnessText}</span>
        </div>
        <div className="dw-kv">
          <span>契约</span>
          <span style={{ color: row.contractValid ? TONE_COLOR.ok : TONE_COLOR.alert }}>
            {detail.contractText}
          </span>
        </div>
      </div>

      <div className="dw-card">
        <div className="dw-sec">
          <h3>体检结论</h3>
          <span>{quality ? (quality.passed ? '通过' : `${quality.findings.length} 项待处理`) : '尚无记录'}</span>
        </div>
        {quality == null ? (
          <p className="dw-muted">这个数据集还没跑过体检，点上面的「重新体检」。</p>
        ) : quality.findings.length === 0 ? (
          <div className="dw-find ok">未发现问题</div>
        ) : (
          quality.findings.map((f, i) => (
            <div key={i} className={`dw-find ${f.severity === 'error' ? 'error' : 'warn'}`}>
              <b>{f.kind}</b> · {f.message}
              {f.count ? `（${f.count}）` : ''}
            </div>
          ))
        )}
      </div>

      <div className="dw-card">
        <div className="dw-sec">
          <h3>覆盖率热力图</h3>
          <span>{cells.length > 0 ? `按小时格子 · ${cells.length} 格` : '暂无数据'}</span>
        </div>
        {cells.length > 0 ? <CoverageHeatmap cells={cells} maxDays={14} /> : null}
      </div>

      <ClearDataZone dataset={dataset} client={client} now={now} onChanged={onChanged} />
    </>
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

function SqlSection({ client, onChanged }: { client: DatahubClient; onChanged: () => void }) {
  const [sql, setSql] = useState(
    'SELECT instrument_id, "interval", count(*) AS bars, min(ts) AS from_ts, max(ts) AS to_ts FROM ohlcv GROUP BY 1,2 ORDER BY bars DESC',
  );
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
    { label: '最近 50 根', sql: 'SELECT * FROM ohlcv ORDER BY ts DESC LIMIT 50' },
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

function EmptyCatalog() {
  return (
    <p className="dw-empty">
      目录为空。先在「数据接入」里确认数据源可用、新建一个接入任务并运行，或用「手动导入」灌一份 CSV；
      有了数据之后这里才会出现台账。
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

function fmtCell(v: unknown): string {
  if (v === null || v === undefined) return '∅';
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

function errMsg(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
  return String(e);
}
