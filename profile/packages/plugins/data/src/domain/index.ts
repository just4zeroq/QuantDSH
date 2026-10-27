// Domain layer — pure logic, zero I/O, zero Host/React imports.
// This module is the single implementation of data semantics shared by the data
// page AND the market (quote) page (QUANTDSH_DATA_RESEARCH.local.md §8.1). It is
// the only place that knows how to normalize a symbol, grade freshness, compute
// coverage gaps, and validate the data contract. Every consumer (UI badge, agent
// tool, backtest loader) MUST import from here, never reimplement.

export * from './symbol';
export * from './market';
export * from './freshness';
export * from './coverage';
export * from './contract';
export * from './dataset';
