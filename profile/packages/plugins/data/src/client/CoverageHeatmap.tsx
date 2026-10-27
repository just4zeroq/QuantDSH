import React, { useMemo } from 'react';
import type { CoverageCell } from '../domain';

/**
 * 覆盖率热力网格（日 × 小时）。
 *
 * 关键的诚实原则：**缺格留空，不填色**。
 * 一个 present=0 的格子必须画成"空"，而不是画成"0% 的深色"。
 * 因为"没数据"和"有数据但为 0"是完全不同的事实 —— 前者要补同步，后者不用。
 */

export interface CoverageHeatmapProps {
  cells: readonly CoverageCell[];
  /** 只显示这些天（避免一个月 31 行太挤）；不传则显示全部 */
  maxDays?: number;
}

const styles = `
.chm-wrap{overflow:auto}
.chm-grid{display:grid;grid-template-columns:repeat(24,1fr);gap:2px;min-width:420px}
.chm-cell{height:14px;border-radius:2px;background:var(--dsw-alias-bg-layer-2,#eef1f5);border:1px solid transparent}
.chm-cell.empty{background:transparent;border-color:var(--dsw-alias-border-l2,#dce2e9)}
.chm-row{display:flex;align-items:center;gap:8px;margin-bottom:3px}
.chm-day{font-size:11px;color:var(--dsw-alias-label-secondary,#657185);width:82px;flex:none;
  font-variant-numeric:tabular-nums}
.chm-legend{display:flex;align-items:center;gap:6px;font-size:11px;
  color:var(--dsw-alias-label-tertiary,#959da6);margin-top:8px}
.chm-sw{width:12px;height:12px;border-radius:2px;display:inline-block}
`;

/** 覆盖率 → 颜色深浅。空数据保持透明（留空），有数据按完整度加深。 */
function cellStyle(ratio: number): React.CSSProperties {
  if (ratio <= 0) return {}; // 由 .empty 类处理
  // 0<ratio<=1 → 从不透明 25% 到 100%，避免满格与半格看起来一样
  const alpha = 0.25 + 0.75 * Math.min(1, ratio);
  return { background: `rgba(43, 164, 113, ${alpha.toFixed(3)})` };
}

export function CoverageHeatmap({ cells, maxDays = 7 }: CoverageHeatmapProps) {
  const days = useMemo(() => {
    const byDay = new Map<string, CoverageCell[]>();
    for (const c of cells) {
      const arr = byDay.get(c.day) ?? [];
      arr.push(c);
      byDay.set(c.day, arr);
    }
    return [...byDay.entries()]
      .sort((a, b) => (a[0] < b[0] ? 1 : -1)) // 最近的在前
      .slice(0, maxDays);
  }, [cells, maxDays]);

  if (!cells.length) {
    return (
      <div className="chm-wrap">
        <style>{styles}</style>
        <p style={{ fontSize: 12, color: 'var(--dsw-alias-label-tertiary,#959da6)' }}>暂无覆盖率数据</p>
      </div>
    );
  }

  return (
    <div className="chm-wrap">
      <style>{styles}</style>
      {days.map(([day, list]) => {
        const sorted = [...list].sort((a, b) => a.hour - b.hour);
        return (
          <div className="chm-row" key={day}>
            <span className="chm-day">{day}</span>
            <div className="chm-grid">
              {sorted.map((c) => (
                <div
                  key={`${day}-${c.hour}`}
                  className={`chm-cell${c.present === 0 ? ' empty' : ''}`}
                  style={cellStyle(c.ratio)}
                  title={`${day} ${String(c.hour).padStart(2, '0')}:00 — ${c.present}/${c.expected}`}
                />
              ))}
            </div>
          </div>
        );
      })}
      <div className="chm-legend">
        <span className="chm-sw" style={{ background: 'rgba(43,164,113,1)' }} />
        <span>满</span>
        <span className="chm-sw" style={{ background: 'rgba(43,164,113,0.4)' }} />
        <span>部分</span>
        <span className="chm-sw" style={{ border: '1px solid #dce2e9' }} />
        <span>缺（留空，不填色）</span>
      </div>
    </div>
  );
}
