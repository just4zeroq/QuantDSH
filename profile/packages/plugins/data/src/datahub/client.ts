/**
 * Rust 守护进程（quantdsh-datahub）的 HTTP 客户端。
 *
 * 为什么走 HTTP 而不是让页面直接 import 引擎：
 * 页面被打包成浏览器 bundle，而引擎依赖 node:sqlite 与原生 duckdb —— 打进浏览器必然失败。
 * 守护进程是唯一的写者（DuckDB 跨进程只允许一个写者），页面作为众多只读消费者之一通过 HTTP 取数。
 * 这条边界同时也让 AI 工具、CLI、其它插件拿到完全相同的语义。
 *
 * 本文件是**同构**的：只依赖 fetch / URL，浏览器与 Node 主进程都能用，
 * 因此插件的 host 侧（Agent 工具）与 client 侧（React 页面）共用同一份实现，
 * 不会出现"页面看到的和工具认为的不一样"。
 *
 * 端点发现顺序：
 *   1. `globalThis.__QUANTDSH_DATAHUB__` —— Tauri sidecar 拉起后由宿主注入（含端口与 token）
 *   2. `VITE_DATAHUB_URL` / `VITE_DATAHUB_TOKEN` —— 本地开发时手工指定
 *   3. 默认 127.0.0.1:17821（开发态直接跑 daemon 即可）
 */

export interface DatahubEndpoint {
  baseUrl: string;
  token: string;
}

export const DEFAULT_DATAHUB_PORT = 17821;

type GlobalWithEndpoint = typeof globalThis & {
  __QUANTDSH_DATAHUB__?: Partial<DatahubEndpoint>;
};

/** 解析端点。浏览器侧永远能解析出一个值（未注入时用默认端口），由调用方处理连不上的情况。 */
export function resolveEndpoint(): DatahubEndpoint {
  const injected = (globalThis as GlobalWithEndpoint).__QUANTDSH_DATAHUB__;
  if (injected?.baseUrl) {
    return { baseUrl: injected.baseUrl, token: injected.token ?? '' };
  }

  // 注：这里**不**读 Vite 的 `import.meta.env`。该客户端会被打成 **CJS** 浏览器包，
  // 套在 `function(require){…}` 里运行；`import.meta` 在非 module 作用域下是语法错误，
  // 会导致整个 client bundle 解析失败、页面白屏。开发态直接本地起 daemon 走默认端口即可，
  // 生产态由 sidecar 把端口与 token 注入 `__QUANTDSH_DATAHUB__` 全局，无需读环境变量。
  return {
    baseUrl: `http://127.0.0.1:${DEFAULT_DATAHUB_PORT}`,
    token: '',
  };
}

/** 同源端点发现路由（由本插件 Host 侧注册，见 src/index.ts）。
 *  为什么需要它：桌面端把端点和 token 注入 `__QUANTDSH_DATAHUB__`，但只要注入缺失
 *  （iframe、二次导航、注入时机早于 sidecar ready），页面就会退到「默认端口 + 空 token」，
 *  而守护进程对 `/api/*` 一律验 token —— 于是表现为 **读取失败 unauthorized**。
 *  走同源 HTTP 向宿主换取端点，令牌不落到 URL、也不跨源可读，安全模型与注入一致。 */
export const DATAHUB_ENDPOINT_PATH = '/api/quantdsh-datahub-endpoint';

/** 注入的端点（存在即用，不额外发请求）。 */
export function resolveInjectedEndpoint(): DatahubEndpoint | null {
  if (injectedDistrusted) return null;
  const injected = (globalThis as GlobalWithEndpoint).__QUANTDSH_DATAHUB__;
  if (injected?.baseUrl) {
    return { baseUrl: injected.baseUrl, token: injected.token ?? '' };
  }
  return null;
}

let discovered: DatahubEndpoint | null = null;
/**
 * 注入值是**窗口创建时**的快照。守护进程重启会换 token，而 webview 不会重建，
 * 于是 `__QUANTDSH_DATAHUB__` 会一直指向旧令牌，重试多少次都还是 401。
 * 一旦鉴权失败就把它标记为不可信，后续发现改走同源路由（读端口文件，永远是最新的）。
 */
let injectedDistrusted = false;

/** 清空端点缓存：token 随守护进程重启而变，401 后要能重新发现。 */
export function resetEndpointCache(options: { distrustInjected?: boolean } = {}): void {
  discovered = null;
  if (options.distrustInjected !== false) injectedDistrusted = true;
}

/**
 * 浏览器侧解析端点（异步）：注入 → 同源路由 → 默认端口。
 * Host（Node）侧不要用它（相对路径 fetch 不成立），用 ./endpoint 的 resolveHostEndpoint。
 */
export async function resolveEndpointAsync(): Promise<DatahubEndpoint> {
  if (discovered) return discovered;
  const injected = resolveInjectedEndpoint();
  if (injected) {
    discovered = injected;
    return injected;
  }
  try {
    const res = await fetch(DATAHUB_ENDPOINT_PATH, {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
    });
    if (res.ok) {
      const body = (await res.json()) as {
        value?: Partial<DatahubEndpoint>;
        baseUrl?: string;
        token?: string;
      };
      const value = body?.value ?? body;
      if (value && typeof value.baseUrl === 'string' && value.baseUrl) {
        discovered = {
          baseUrl: value.baseUrl,
          token: typeof value.token === 'string' ? value.token : '',
        };
        return discovered;
      }
    }
  } catch {
    /* 非 web 宿主（CLI / 测试）没有这条路由：回落到默认端口 */
  }
  discovered = { baseUrl: `http://127.0.0.1:${DEFAULT_DATAHUB_PORT}`, token: '' };
  return discovered;
}

export class DatahubError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(message: string, status: number, code: string) {
    super(message);
    this.name = 'DatahubError';
    this.status = status;
    this.code = code;
  }
}

async function request<T>(
  endpoint: DatahubEndpoint,
  path: string,
  init: { method?: string; body?: unknown; query?: Record<string, string | number | undefined> } = {},
): Promise<T> {
  const url = new URL(`${endpoint.baseUrl}${path}`);
  if (init.query) {
    for (const [k, v] of Object.entries(init.query)) {
      if (v !== undefined && v !== '') url.searchParams.set(k, String(v));
    }
  }

  const headers: Record<string, string> = { Accept: 'application/json' };
  if (endpoint.token) headers.Authorization = `Bearer ${endpoint.token}`;
  if (init.body !== undefined) headers['Content-Type'] = 'application/json';

  let res: Response;
  try {
    res = await fetch(url.toString(), {
      method: init.method ?? 'GET',
      headers,
      body: init.body === undefined ? undefined : JSON.stringify(init.body),
    });
  } catch {
    // 连接不上是最常见的状态（守护进程没起），要给出可行动的提示，不是堆栈。
    throw new DatahubError(`无法连接数据服务 ${endpoint.baseUrl}`, 0, 'unreachable');
  }

  if (!res.ok) {
    let code = 'http_error';
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = (await res.json()) as { error?: { code?: string; message?: string } };
      if (body.error?.message) message = body.error.message;
      if (body.error?.code) code = body.error.code;
    } catch {
      /* 非 JSON 错误响应，保留状态码信息 */
    }
    throw new DatahubError(message, res.status, code);
  }

  return (await res.json()) as T;
}

// ────────────────────────────── 契约类型 ──────────────────────────────
// 字段与 Rust 侧的 serde 命名严格对应。Rust 结构体没有 rename_all，
// 线上格式就是 snake_case —— 客户端不要自作主张转 camelCase，
// 否则一次改名就会让"页面能跑、Agent 工具取不到值"。

export interface DaemonHealth {
  status: string;
  startedAt: number;
  dataDir: string;
}

/** 目录里的一条数据集（Rust metadata::DatasetRecord）。 */
export interface DatasetWire {
  dataset_key: string;
  market: string;
  exchange: string;
  symbol: string;
  interval: string;
  source: string;
  namespace: string;
  as_of: number | null;
  ingested_at: number | null;
  closed_only: boolean;
  row_count: number;
  created_at: number;
  updated_at: number;
}

export interface CoverageCellWire {
  day: string;
  hour: number;
  present: number;
  expected: number;
  ratio: number;
}

export interface CoverageSummaryWire {
  present: number;
  expected: number;
  ratio: number;
}

export interface SyncJobWire {
  id: string;
  dataset_key: string;
  source: string;
  symbols: string[];
  interval: string;
  cron: string | null;
  mode: string;
  cursor: number | null;
  status: string;
  last_run_at: number | null;
  last_error: string | null;
  rows_written: number;
  enabled: boolean;
  created_at: number;
}

export interface SyncOutcomeWire {
  job_id: string;
  status: string;
  rows_written: number;
  cursor: number | null;
  from: number;
  to: number;
  error: string | null;
}

export interface QueryResultWire {
  status: 'ok' | 'blocked' | 'error';
  columns: string[];
  rows: Record<string, unknown>[];
  truncated: boolean;
  duration_ms: number;
  reason?: string;
  code?: string;
  message?: string;
}

export interface TableMetaWire {
  table: string;
  columns: { column: string; type: string; nullable: boolean }[];
}

// ────────────────────────────── 写入 / 质量 契约类型 ──────────────────────────────
// 与 Rust 侧 serde 命名严格对应（snake_case 线上格式），不要自作主张转 camelCase。

export interface ImportRecordWire {
  id: string;
  dataset_key: string;
  namespace: string;
  file: string;
  rows_total: number;
  rows_ok: number;
  rows_bad: number;
  gate_passed: boolean;
  gate_report?: unknown;
  created_at: number;
}

export interface CsvImportRequestWire {
  dataset_key: string;
  symbol: string;
  interval: string;
  file: string;
  text: string;
  namespace?: string;
  source?: string;
  max_bad_ratio?: number;
}

export interface CsvImportResultWire {
  batch_id: string;
  rows_total: number;
  rows_ok: number;
  rows_bad: number;
  gate_passed: boolean;
  written: number;
  findings: QualityFindingWire[];
  bad_samples: string[];
}

export interface PromoteRequestWire {
  dataset_key: string;
  symbol: string;
  interval: string;
  source?: string;
}

export interface PromoteResultWire {
  moved: number;
  min_ts: number | null;
  max_ts: number | null;
}

export interface QualityFindingWire {
  kind: string;
  severity: string; // error | warn
  message: string;
  count: number;
}

export interface QualityOptionsWire {
  symbol: string;
  interval: string;
  from_ts: number;
  to_ts: number;
  namespace?: string;
  now?: number;
  max_stale_ms?: number;
}

export interface QualityReportWire {
  findings: QualityFindingWire[];
  passed: boolean;
  expected: number;
  present: number;
  coverage_ratio: number;
  min_ts: number | null;
  max_ts: number | null;
}

export interface JobPatchWire {
  status?: string;
  cursor?: number | null;
  last_run_at?: number | null;
  last_error?: string | null;
  rows_written?: number;
  cron?: string | null;
  enabled?: boolean;
}

export interface CorrectBodyWire {
  symbol: string;
  interval: string;
  ts: number;
  namespace?: string;
  open?: number;
  high?: number;
  low?: number;
  close?: number;
  volume?: number;
  turnover?: number;
}

export interface DeleteBarsWire {
  namespace?: string;
  symbol: string;
  interval: string;
  from: number;
  to: number;
}

export interface CsvImportBody {
  dataset_key: string;
  symbol: string;
  interval: string;
  file: string;
  text: string;
  namespace?: string;
  source?: string;
  max_bad_ratio?: number;
}

export interface DatahubClient {
  health(): Promise<DaemonHealth>;
  /** `/health` 免鉴权，用来判定"守护进程活着但没有 token / token 不对"。 */
  probe(): Promise<DaemonHealth>;
  datasets(market?: string, namespace?: string): Promise<DatasetWire[]>;
  bars(q: {
    symbol?: string;
    interval?: string;
    namespace?: string;
    from?: number;
    to?: number;
    limit?: number;
  }): Promise<Record<string, unknown>[]>;
  timestamps(q: {
    symbol: string;
    interval: string;
    namespace?: string;
    from?: number;
    to?: number;
  }): Promise<number[]>;
  refreshDatasetStats(datasetKey: string): Promise<void>;
  coverage(datasetKey: string): Promise<{ datasetKey: string; cells: CoverageCellWire[] }>;
  coverageSummary(datasetKey: string): Promise<CoverageSummaryWire>;
  refreshCoverage(datasetKey: string, year: number, month: number): Promise<CoverageCellWire[]>;
  jobs(enabledOnly?: boolean, datasetKey?: string): Promise<SyncJobWire[]>;
  createJob(job: unknown): Promise<{ id: string }>;
  runJob(
    id: string,
    body?: { dry_run?: boolean; now?: number; limit?: number },
  ): Promise<SyncOutcomeWire>;
  schema(): Promise<TableMetaWire[]>;
  query(sql: string, maxRows?: number): Promise<QueryResultWire>;
  queryHistory(limit?: number): Promise<Record<string, unknown>[]>;
  sources(): Promise<string[]>;
  markets(): Promise<string[]>;
  updateJob(id: string, patch: JobPatchWire): Promise<void>;
  deleteJob(id: string): Promise<void>;
  importCsv(req: CsvImportBody): Promise<CsvImportResultWire>;
  promote(req: PromoteRequestWire): Promise<PromoteResultWire>;
  listImports(datasetKey?: string): Promise<ImportRecordWire[]>;
  quality(opts: QualityOptionsWire): Promise<QualityReportWire>;
  latestQuality(datasetKey: string): Promise<QualityReportWire | null>;
  correctBar(body: CorrectBodyWire): Promise<{ changed: boolean }>;
  deleteBars(body: DeleteBarsWire): Promise<{ deleted: number }>;
}

export function createDatahubClient(endpoint: DatahubEndpoint): DatahubClient {
  return {
    health: () => request<DaemonHealth>(endpoint, '/health'),
    probe: () => request<DaemonHealth>(endpoint, '/health'),
    datasets: (market, namespace) =>
      request<DatasetWire[]>(endpoint, '/api/datasets', {
        query: { market, namespace },
      }),
    bars: (q) =>
      request<Record<string, unknown>[]>(endpoint, '/api/bars', {
        query: {
          symbol: q.symbol,
          interval: q.interval,
          namespace: q.namespace,
          from: q.from,
          to: q.to,
          limit: q.limit,
        },
      }),
    timestamps: (q) =>
      request<{ timestamps: number[] }>(endpoint, '/api/bars/timestamps', {
        query: {
          symbol: q.symbol,
          interval: q.interval,
          namespace: q.namespace,
          from: q.from,
          to: q.to,
        },
      }).then((r) => r.timestamps),
    refreshDatasetStats: (datasetKey) =>
      request(endpoint, '/api/datasets/refresh', {
        method: 'POST',
        body: { dataset_key: datasetKey },
      }),
    coverage: (datasetKey) =>
      request(endpoint, '/api/coverage', { query: { dataset_key: datasetKey } }),
    coverageSummary: (datasetKey) =>
      request<CoverageSummaryWire>(endpoint, '/api/coverage/summary', {
        query: { dataset_key: datasetKey },
      }),
    refreshCoverage: (datasetKey, year, month) =>
      request<{ cells: CoverageCellWire[] }>(endpoint, '/api/coverage/refresh', {
        method: 'POST',
        body: { dataset_key: datasetKey, year, month },
      }).then((r) => r.cells),
    jobs: (enabledOnly, datasetKey) =>
      request<SyncJobWire[]>(endpoint, '/api/jobs', {
        query: { enabled_only: enabledOnly ? 'true' : undefined, dataset_key: datasetKey },
      }),
    createJob: (job) => request(endpoint, '/api/jobs', { method: 'POST', body: job }),
    runJob: (id, body) =>
      request<SyncOutcomeWire>(endpoint, `/api/jobs/${encodeURIComponent(id)}/run`, {
        method: 'POST',
        body: body ?? {},
      }),
    schema: () => request<TableMetaWire[]>(endpoint, '/api/schema'),
    query: (sql, maxRows) =>
      request<QueryResultWire>(endpoint, '/api/query', {
        method: 'POST',
        body: { sql, max_rows: maxRows },
      }),
    queryHistory: (limit) =>
      request<{ history: Record<string, unknown>[] }>(endpoint, '/api/query/history', {
        query: { limit },
      }).then((r) => r.history),
    sources: () => request<{ sources: string[] }>(endpoint, '/api/sources').then((r) => r.sources),
    markets: () => request<{ markets: string[] }>(endpoint, '/api/markets').then((r) => r.markets),
    updateJob: (id, patch) =>
      request(endpoint, `/api/jobs/${encodeURIComponent(id)}`, { method: 'POST', body: patch }),
    deleteJob: (id) =>
      request(endpoint, `/api/jobs/${encodeURIComponent(id)}`, { method: 'DELETE' }),
    importCsv: (req) =>
      request<CsvImportResultWire>(endpoint, '/api/import/csv', { method: 'POST', body: req }),
    promote: (req) =>
      request<PromoteResultWire>(endpoint, '/api/import/promote', { method: 'POST', body: req }),
    listImports: (datasetKey) =>
      request<{ imports: ImportRecordWire[] }>(endpoint, '/api/imports', {
        query: { dataset_key: datasetKey },
      }).then((r) => r.imports),
    quality: (opts) => request<QualityReportWire>(endpoint, '/api/quality', { method: 'POST', body: opts }),
    latestQuality: (datasetKey) =>
      request<{ run: QualityReportWire | null }>(endpoint, '/api/quality/latest', {
        query: { dataset_key: datasetKey },
      }).then((r) => r.run),
    correctBar: (body) =>
      request<{ changed: boolean }>(endpoint, '/api/bars/correct', { method: 'POST', body }),
    deleteBars: (body) =>
      request<{ deleted: number }>(endpoint, '/api/bars', {
        method: 'DELETE',
        query: {
          namespace: body.namespace,
          symbol: body.symbol,
          interval: body.interval,
          from: body.from,
          to: body.to,
        },
      }),
  };
}
