import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Vela } from '@luxalgo/vela';
import type { ChartBar, Interval } from './domain';
import { MARKET_LABEL, INTERVALS, toVelaTimeframe } from './domain';
import {
  createDatahubClient,
  resolveEndpointAsync,
  resetEndpointCache,
  DatahubError,
  type DatahubClient,
  type DatasetWire,
  type BarWire,
} from './datahub/client';

// Vela™ (LuxAlgo, Apache-2.0) 的图例水印默认开启，按许可要求保留（等价署名）。
// 这里直接用其离线 `data` 通道：把守护进程里取出的 OHLCV 喂给图表，不接任何外部行情源。
type VelaChart = InstanceType<typeof Vela>;

const styles = `
.mkt-root{height:100%;min-height:0;display:flex;flex-direction:column;width:100%;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129);font:inherit}
.mkt-bar{display:flex;gap:10px;align-items:center;flex-wrap:wrap;padding:10px 14px;border-bottom:1px solid var(--dsw-alias-border-l2,#dce2e9);flex:none}
.mkt-bar label{font-size:13px;color:var(--dsw-alias-label-secondary,#657185)}
.mkt-select,.mkt-input{font:inherit;font-size:13px;padding:5px 8px;border-radius:7px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129)}
.mkt-chart{flex:1;min-height:0;position:relative}
.mkt-chart > div{position:absolute;inset:0}
.mkt-status{padding:8px 14px;font-size:12px;color:var(--dsw-alias-label-tertiary,#959da6);flex:none;border-top:1px solid var(--dsw-alias-border-l2,#dce2e9);display:flex;justify-content:space-between;gap:12px}
.mkt-err{margin:0;padding:10px 14px;font-size:13px;color:var(--dsw-alias-status-error,#d54941);background:rgba(213,73,65,.06);flex:none}
.mkt-empty{padding:24px;font-size:13px;color:var(--dsw-alias-label-secondary,#657185);line-height:1.6}
.mkt-btn{font:inherit;font-size:12px;padding:5px 12px;border-radius:7px;border:1px solid var(--dsw-alias-border-l2,#dce2e9);background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#1d2129);cursor:pointer}
.mkt-btn.primary{background:var(--dsw-alias-brand,#3a6df0);border-color:var(--dsw-alias-brand,#3a6df0);color:#fff}
.mkt-btn:disabled{opacity:.5;cursor:not-allowed}
`;

function toChartBar(b: BarWire): ChartBar {
  return {
    time: b.ts,
    open: b.open,
    high: b.high,
    low: b.low,
    close: b.close,
    volume: b.volume,
  };
}

/**
 * 端点可能是注入的（窗口创建时的快照）也可能来自同源发现路由。
 * 守护进程重启会轮换 token，注入值随之失效；遇到 401/403 就弃用注入值、
 * 重新发现一次再试——这样刷新页面就能自愈，不需要重启整个应用。
 */
async function withAuthRetry<T>(run: (client: DatahubClient) => Promise<T>): Promise<T> {
  const endpoint = await resolveEndpointAsync();
  try {
    return await run(createDatahubClient(endpoint));
  } catch (error) {
    if (error instanceof DatahubError && (error.status === 401 || error.status === 403)) {
      resetEndpointCache();
      return run(createDatahubClient(await resolveEndpointAsync()));
    }
    throw error;
  }
}

export function MarketPage({ nowTs: _nowTs }: { nowTs: number }) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<VelaChart | null>(null);
  const [chartReady, setChartReady] = useState(false);

  const [datasets, setDatasets] = useState<DatasetWire[]>([]);
  const [loadingCatalog, setLoadingCatalog] = useState(true);
  const [catalogError, setCatalogError] = useState<string | null>(null);

  const [symbol, setSymbol] = useState<string>('');
  const [intervalSel, setIntervalSel] = useState<Interval>('1h');
  const [loadingBars, setLoadingBars] = useState(false);
  const [barsError, setBarsError] = useState<string | null>(null);
  const [barCount, setBarCount] = useState<number>(0);
  const [refreshNonce, setRefreshNonce] = useState(0);

  // 把目录按 symbol 聚合，得到可选项；每个 symbol 下挂它已有的周期。
  const bySymbol = useMemo(() => {
    const map = new Map<string, { market: string; intervals: Set<string> }>();
    for (const d of datasets) {
      const cur = map.get(d.symbol) ?? { market: d.market, intervals: new Set<string>() };
      cur.intervals.add(d.interval);
      map.set(d.symbol, cur);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [datasets]);

  // 当前 symbol 可用的周期（用于联动 selector）。
  const availableIntervals = useMemo(() => {
    const entry = bySymbol.find(([s]) => s === symbol);
    return entry ? [...entry[1].intervals] : [];
  }, [bySymbol, symbol]);

  // ── 1) 创建图表实例（仅一次） ──
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    let chart: VelaChart;
    try {
      chart = new Vela(el, { theme: 'dark', timeframe: '1h' });
    } catch (e) {
      setBarsError(e instanceof Error ? `图表初始化失败：${e.message}` : '图表初始化失败');
      return;
    }
    chartRef.current = chart;
    setChartReady(true);

    const ro = new ResizeObserver(() => {
      try {
        chart.resize();
      } catch {
        /* chart 已销毁 */
      }
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      try {
        chart.destroy();
      } catch {
        /* noop */
      }
      chartRef.current = null;
      setChartReady(false);
    };
  }, []);

  // ── 2) 装载目录（标的是从守护进程里真实存在的数据集） ──
  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoadingCatalog(true);
      setCatalogError(null);
      try {
        const list = await withAuthRetry((client) => client.datasets());
        if (cancelled) return;
        setDatasets(list);
        if (list.length > 0) {
          const first = list[0];
          setSymbol(first.symbol);
          setIntervalSel((first.interval as Interval) ?? '1h');
        }
      } catch (e) {
        if (cancelled) return;
        const authFailure = e instanceof DatahubError && (e.status === 401 || e.status === 403);
        setCatalogError(
          e instanceof DatahubError && e.code === 'unreachable'
            ? '数据服务未连接：请先启动 quantdsh-datahub（cargo run -p quantdsh-datahub）'
            : authFailure
              ? '数据服务拒绝了访问令牌：请刷新页面；若仍失败，请重新打开 QuantDSH。'
              : e instanceof Error
                ? e.message
                : '目录加载失败',
        );
      } finally {
        if (!cancelled) setLoadingCatalog(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // ── 3) 选择变化时，拉取 bars 并推给图表 ──
  useEffect(() => {
    if (!chartReady || !symbol) return;
    let cancelled = false;
    (async () => {
      setLoadingBars(true);
      setBarsError(null);
      try {
        // 默认取最近 ~5000 根 bar（覆盖多数周期的较完整历史）。
        const to = Date.now();
        const from = to - 365 * 86_400_000;
        const bars: BarWire[] = await withAuthRetry((client) =>
          client.bars({
            symbol,
            interval: intervalSel,
            from,
            to,
            limit: 5000,
          }),
        );
        if (cancelled) return;
        const chartBars = bars.map(toChartBar).sort((a, b) => a.time - b.time);
        const chart = chartRef.current;
        if (chart) {
          await chart.setMarket({
            symbol,
            timeframe: toVelaTimeframe(intervalSel),
            data: chartBars,
          });
        }
        setBarCount(chartBars.length);
      } catch (e) {
        if (cancelled) return;
        setBarsError(e instanceof Error ? e.message : '行情加载失败');
      } finally {
        if (!cancelled) setLoadingBars(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [chartReady, symbol, intervalSel, refreshNonce]);

  // 当切换 symbol 时，若当前区间不在该 symbol 可用区间里，回退到第一个可用区间。
  function onPickSymbol(next: string) {
    setSymbol(next);
    const entry = bySymbol.find(([s]) => s === next);
    if (entry && !entry[1].intervals.has(intervalSel)) {
      const firstIv = [...entry[1].intervals][0] as Interval | undefined;
      if (firstIv) setIntervalSel(firstIv);
    }
  }

  return (
    <div className="mkt-root">
      <style>{styles}</style>
      <div className="mkt-bar">
        <label>标的</label>
        <select className="mkt-select" value={symbol} onChange={(e) => onPickSymbol(e.target.value)}>
          {bySymbol.length === 0 ? (
            <option value="">（无数据集）</option>
          ) : (
            bySymbol.map(([s, meta]) => (
              <option key={s} value={s}>
                {s} · {MARKET_LABEL[(meta.market as keyof typeof MARKET_LABEL) ?? 'CRYPTO'] ?? meta.market}
              </option>
            ))
          )}
        </select>
        <label>周期</label>
        <select
          className="mkt-select"
          value={intervalSel}
          onChange={(e) => setIntervalSel(e.target.value as Interval)}
        >
          {(availableIntervals.length ? availableIntervals : INTERVALS).map((iv) => (
            <option key={iv} value={iv}>
              {iv}
            </option>
          ))}
        </select>
        <button
          className="mkt-btn primary"
          disabled={!symbol || loadingBars}
          onClick={() => {
            // 触发一次重新加载：递增 nonce 以走 effect（symbol/interval 未变时也能刷新）。
            setRefreshNonce((n) => n + 1);
          }}
        >
          {loadingBars ? '加载中…' : '刷新'}
        </button>
        <span style={{ fontSize: 12, color: 'var(--dsw-alias-label-tertiary,#959da6)' }}>
          {loadingCatalog ? '目录加载中…' : `共 ${bySymbol.length} 个标的`}
        </span>
      </div>

      {catalogError && <p className="mkt-err">{catalogError}</p>}
      {!catalogError && !loadingCatalog && bySymbol.length === 0 && (
        <div className="mkt-empty">
          目录为空。先用「数据」工作台创建数据集并同步，或在该工作台用 CSV 导入行情，
          再回到这里画图。
        </div>
      )}

      <div className="mkt-chart">
        <div ref={containerRef} aria-label="行情图表" />
      </div>

      <div className="mkt-status">
        <span>{symbol ? `${symbol} · ${intervalSel}` : '未选择标的'}</span>
        <span>{barsError ? `错误：${barsError}` : `已绘制 ${barCount} 根 K 线`}</span>
      </div>
    </div>
  );
}
