import type { Context } from '@deepseek-ai/cordis';
import type { HostConnectionHandle } from '@deepseek-ai/dsh-client-connection';
import type { WorkbenchDescriptor } from 'quantdsh-contracts';
import {
  createDatahubClient,
  DATAHUB_ENDPOINT_PATH,
  type DatahubClient,
  type DatahubEndpoint,
} from './datahub/client';
import { resolveHostEndpoint } from './datahub/endpoint';
import { loadDatasetsFromDatahub } from './datahub/adapter';

export const name = 'quantdsh-data';
/** 数据工坊是内置业务页，由 Client 侧直接注册面板，不再走工作台注册链，
 * 因此 Host 侧不注入 `quantdshWorkbenchRegistration`。 */
export const inject: string[] = [];
/** 保留工作台声明：package.json 的 `quantdsh.workbench` 仍与之一致，
 * 供目录/清单核对使用，但当前不发起注册。 */
export const descriptor: WorkbenchDescriptor = {
  id: 'quantdsh.data',
  title: '数据',
  description: '数据集目录、覆盖率与同步任务',
  protocolVersion: '1.0',
  layout: 'full-page',
  hostCompatibility: { sdk: '^1.0.0', harness: '0.2.1-rc.2' },
  capabilities: [],
};

let clientRef: DatahubClient | null = null;

/**
 * 数据服务客户端（懒解析端点）。
 *
 * 为什么这里**不**再持有数据库：
 * 行情在 DuckDB 里，而 DuckDB 跨进程只允许一个写者。如果每个插件/每个工具各开一份
 * 连接，就必然在某个时刻撞上"文件已被锁定"。所以全部后端能力收进 Rust 守护进程
 * （`quantdsh-datahub`），主进程与页面一样只是 HTTP 消费者 —— 这一条同时让
 * AI 工具、CLI、页面拿到完全相同的语义。
 *
 * 端点解析顺序见 ./datahub/endpoint.ts：环境变量 → 守护进程端口文件 → 默认端口。
 */
export function getDatahub(endpoint: DatahubEndpoint = resolveHostEndpoint()): DatahubClient {
  if (!clientRef) clientRef = createDatahubClient(endpoint);
  return clientRef;
}

/**
 * 把守护进程端点（端口 + token）以**同源 HTTP** 下发给页面。
 *
 * 为什么不只靠 webview 注入：注入依赖「创建窗口时 sidecar 已就绪」，一旦注入缺失，
 * 页面只能退到「默认端口 + 空 token」，而 `/api/*` 一律验 token —— 表现为
 * `读取失败 unauthorized`。这条路由由 Connection 的 peer 信任策略保护
 * （只有 owner peer 可达），令牌也不会出现在 URL 里，安全性等同注入。
 */
function registerEndpointRoute(ctx: Context): void {
  // 用作用域注入而不是把 'connection' 写进插件级 inject：这条路由是**可选**能力，
  // 宿主没有 web 通道（纯 CLI 调度）时不该让整个数据插件加载失败。
  ctx.inject(['connection'], (scope) => {
    const connection = scope.get('connection') as HostConnectionHandle | undefined;
    const registry = connection?.fetch;
    if (!registry || typeof registry.register !== 'function') return;
    const unregister = registry.register({
      path: DATAHUB_ENDPOINT_PATH,
      methods: ['GET'],
      requestBody: 'buffered',
      fetch: async () =>
        Response.json(
          { ok: true, value: resolveHostEndpoint() },
          { headers: { 'cache-control': 'no-store' } },
        ),
    });
    scope.effect(() => unregister, 'quantdsh.data.endpoint-route');
  });
}

export async function apply(ctx: Context) {
  // 页面注册已移到 Client 入口（src/client.tsx）；Host 只发布数据服务。
  // 把客户端暴露给同进程其它插件 / Agent 工具，避免各处各拼一套 URL 与错误处理。
  // cordis 要求服务用 provide 发布（set 只允许修改已有属性）。
  ctx.provide('quantdshDataHub', {
    client: () => getDatahub(),
    /** 目录 + 同步任务，已适配成 domain 层的 DatasetRecord（与页面同一口径） */
    datasets: async (now = Date.now()) => loadDatasetsFromDatahub(now),
    /** 供调度器调用：挑出到点的定时任务 */
    due: async (now = Date.now()) => (await getDatahub().jobs(true)).length,
  });
  registerEndpointRoute(ctx);

  // 返回清理回调：cordis 在插件卸载（dispose）时会调用，释放缓存的客户端。
  return () => {
    clientRef = null;
  };
}
