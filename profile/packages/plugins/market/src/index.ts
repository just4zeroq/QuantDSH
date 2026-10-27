import type { Context } from '@deepseek-ai/cordis';
import type { WorkbenchDescriptor } from 'quantdsh-contracts';
import { createDatahubClient, type DatahubClient, type DatahubEndpoint } from './datahub/client';
import { resolveHostEndpoint } from './datahub/endpoint';

export const name = 'quantdsh-market';
export const inject = ['quantdshWorkbenchRegistration'];

export const descriptor: WorkbenchDescriptor = {
  id: 'quantdsh.market',
  title: '行情',
  description: 'K 线行情看盘（看盘/指标双视图，数据来自本地行情库）',
  protocolVersion: '1.0',
  layout: 'full-page',
  hostCompatibility: { sdk: '^1.0.0', harness: '0.2.1-rc.2' },
  capabilities: [],
};

let clientRef: DatahubClient | null = null;

/**
 * 数据服务客户端（懒解析端点）。行情页与数据工坊共用同一个本地行情服务：
 * 服务是 DuckDB 的唯一写者，主进程/页面都是只读消费者。
 */
export function getDatahub(endpoint: DatahubEndpoint = resolveHostEndpoint()): DatahubClient {
  if (!clientRef) clientRef = createDatahubClient(endpoint);
  return clientRef;
}

export async function apply(ctx: Context) {
  const registration = ctx.get('quantdshWorkbenchRegistration') as {
    registerManaged(caller: Context, declaration: WorkbenchDescriptor, page: string): Promise<unknown>;
  };
  await registration.registerManaged(ctx, descriptor, 'quantdsh-market-page');

  // 把客户端暴露给同进程其它插件 / Agent 工具。
  // cordis 要求服务用 provide 发布（set 只允许修改已有属性）。
  ctx.provide('quantdshMarketDataHub', {
    client: () => getDatahub(),
    datasets: async () => getDatahub().datasets(),
  });

  // 返回清理回调：cordis 在插件卸载（dispose）时会调用，释放缓存的客户端。
  return () => {
    clientRef = null;
  };
}
