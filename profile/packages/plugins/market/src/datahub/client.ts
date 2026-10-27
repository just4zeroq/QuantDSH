/**
 * 本地行情服务的 HTTP 客户端 —— 行情页的只读消费端。
 *
 * 与数据工坊共用同一份契约：行情存在服务自己的 DuckDB 里，服务是唯一写者，
 * 页面只是众多只读消费者之一。这里只取两类数据：
 *   - 目录（datasets / markets）：用来填充标的与周期选择器
 *   - bars：画 K 线用的 OHLCV
 *
 * 本文件是**同构**的：只依赖 fetch / URL，浏览器与主进程都能用，因此 host 侧的
 * 工具与 client 侧的页面拿到完全相同的语义。
 *
 * 端点发现顺序：
 *   1. `globalThis.__QUANTDSH_DATAHUB__` —— Tauri sidecar 拉起后由宿主注入
 *   2. `VITE_DATAHUB_URL` / `VITE_DATAHUB_TOKEN` —— 本地开发手工指定
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
  return {
    baseUrl: `http://127.0.0.1:${DEFAULT_DATAHUB_PORT}`,
    token: '',
  };
}

/** 同源端点发现路由（由数据插件的 Host 侧注册，见 quantdsh-plugin-data 的 src/index.ts）。
 *  注入缺失时（窗口早于守护进程创建、二次导航）页面只能退到「默认端口 + 空 token」，
 *  而守护进程对 `/api/*` 一律验 token，表现为读取失败 unauthorized。 */
export const DATAHUB_ENDPOINT_PATH = '/api/quantdsh-datahub-endpoint';

let discovered: DatahubEndpoint | null = null;
/** 注入值是窗口创建时的快照：守护进程重启换 token 后它就永久过期，必须能弃用。 */
let injectedDistrusted = false;

export function resetEndpointCache(options: { distrustInjected?: boolean } = {}): void {
  discovered = null;
  if (options.distrustInjected !== false) injectedDistrusted = true;
}

/** 浏览器侧异步解析：注入 → 同源路由 → 默认端口。Host 侧不要用（相对路径不成立）。 */
export async function resolveEndpointAsync(): Promise<DatahubEndpoint> {
  if (discovered) return discovered;
  if (!injectedDistrusted) {
    const injected = resolveEndpoint();
    const raw = (globalThis as GlobalWithEndpoint).__QUANTDSH_DATAHUB__;
    if (raw?.baseUrl) {
      discovered = injected;
      return injected;
    }
  }
  try {
    const res = await fetch(DATAHUB_ENDPOINT_PATH, {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' },
    });
    if (res.ok) {
      const body = (await res.json()) as { value?: Partial<DatahubEndpoint>; baseUrl?: string; token?: string };
      const value = body?.value ?? body;
      if (value && typeof value.baseUrl === 'string' && value.baseUrl) {
        discovered = { baseUrl: value.baseUrl, token: typeof value.token === 'string' ? value.token : '' };
        return discovered;
      }
    }
  } catch {
    /* 非 web 宿主没有这条路由：回落到默认端口 */
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

// ────────────────────────────── 契约类型（与 Rust 侧 serde snake_case 严格对应） ──────────────────────────────

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

/** 一根 K 线（Rust market::Bar 的线上形态：ts 为 ms epoch）。 */
export interface BarWire {
  symbol: string;
  interval: string;
  ts: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
  turnover: number;
  closed: boolean;
  source: string;
  namespace: string;
  ingested_at: number;
}

export interface DatahubClient {
  health(): Promise<{ status: string }>;
  datasets(market?: string, namespace?: string): Promise<DatasetWire[]>;
  markets(): Promise<string[]>;
  bars(q: {
    symbol?: string;
    interval?: string;
    namespace?: string;
    from?: number;
    to?: number;
    limit?: number;
  }): Promise<BarWire[]>;
}

export function createDatahubClient(endpoint: DatahubEndpoint): DatahubClient {
  return {
    health: () => request<{ status: string }>(endpoint, '/health'),
    datasets: (market, namespace) =>
      request<DatasetWire[]>(endpoint, '/api/datasets', {
        query: { market, namespace },
      }),
    markets: () =>
      request<{ markets: string[] }>(endpoint, '/api/markets').then((r) => r.markets),
    bars: (q) =>
      request<BarWire[]>(endpoint, '/api/bars', {
        query: {
          symbol: q.symbol,
          interval: q.interval,
          namespace: q.namespace,
          from: q.from,
          to: q.to,
          limit: q.limit,
        },
      }),
  };
}
