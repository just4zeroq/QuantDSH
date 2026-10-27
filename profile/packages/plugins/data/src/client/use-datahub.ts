/**
 * React 装载钩子：把守护进程的目录 + 元数据拉进来并适配成 domain 的 DatasetRecord。
 *
 * 适配逻辑本身在 ../datahub/adapter（那里不依赖 React，主进程与 Agent 工具复用同一份）。
 * 这里只负责生命周期与**状态建模**，并为各功能面板提供统一的 `client` 与 `refresh`。
 */

import { useCallback, useEffect, useState } from 'react';
import {
  createDatahubClient,
  resolveEndpointAsync,
  resetEndpointCache,
  DatahubError,
  type DatahubClient,
  type DatahubEndpoint,
  type DatasetWire,
  type SyncJobWire,
  type TableMetaWire,
} from '../datahub/client';
import { loadDatasetsFromDatahub } from '../datahub/adapter';
import type { DatasetRecord } from '../domain';

// 供工作台组件复用同一客户端类型（DataWorkbench 从这里取）。
export type { DatahubClient } from '../datahub/client';

export interface DatahubSchemaTable {
  table: string;
  columns: { column: string; type: string; nullable: boolean }[];
}

export interface DatahubMeta {
  datasets: DatasetRecord[];
  /** 守护进程的原始目录行（含 namespace / row_count / 数据源），页面表格与行情看板要用，
   *  domain 的 DatasetRecord 只保留判定所需的字段，不该为展示再扩宽契约。 */
  wire: DatasetWire[];
  jobs: SyncJobWire[];
  sources: string[];
  markets: string[];
  schema: TableMetaWire[];
}

export type DatahubState =
  | { kind: 'loading' }
  | { kind: 'unreachable'; endpoint: string; message: string }
  | { kind: 'error'; message: string }
  | { kind: 'ready'; client: DatahubClient; meta: DatahubMeta };

/**
 * 鉴权失败的提示必须能指导下一步动作，而不是「失败了」。
 * 两种情况的原因完全不同：没拿到令牌（宿主没下发）与令牌过期（守护进程重启轮换）。
 */
function authMessage(endpoint: DatahubEndpoint | null): string {
  const at = endpoint?.baseUrl ? `（${endpoint.baseUrl}）` : '';
  if (!endpoint?.token) {
    return `数据服务需要访问令牌，但页面没有取得令牌${at}。请完全退出 QuantDSH 后重新打开（宿主会随窗口一起下发令牌）。`;
  }
  return `数据服务拒绝了页面的访问令牌${at}。数据服务重启会轮换令牌，请点「重试」或重新打开 QuantDSH。`;
}

/**
 * 失败态被显式建模而不是抛异常：守护进程没起是**正常状态**（用户还没开应用、
 * sidecar 还在启动），页面要给出"去启动服务"的可行动提示，而不是红屏堆栈。
 */
export function useDatahubWorkbench(
  nowTs: number,
  endpointOverride?: DatahubEndpoint,
): DatahubState & { reload: () => void } {
  const [state, setState] = useState<DatahubState>({ kind: 'loading' });

  const reload = useCallback(() => {
    let cancelled = false;
    const endpointHolder: { value: DatahubEndpoint | null } = { value: endpointOverride ?? null };
    void (async () => {
      try {
        // 端点可能来自注入、同源路由或默认端口 —— 必须异步解析。
        // 旧实现同步回落到「默认端口 + 空 token」，守护进程一律验 token，
        // 于是页面稳定拿到 401 unauthorized。
        const endpoint = endpointOverride ?? (await resolveEndpointAsync());
        endpointHolder.value = endpoint;

        const load = async (ep: DatahubEndpoint) => {
          const client = createDatahubClient(ep);
          await client.health(); // /health 免鉴权：先分清"没起"还是"token 不对"
          const payload = await loadDatasetsFromDatahub(nowTs, ep);
          const settle = async <T>(run: () => Promise<T>, fallback: T): Promise<T> => {
            try {
              return await run();
            } catch {
              return fallback;
            }
          };
          const [wire, sources, markets, schema] = await Promise.all([
            settle<DatasetWire[]>(() => client.datasets(), []),
            settle<string[]>(() => client.sources(), []),
            settle<string[]>(() => client.markets(), []),
            settle<TableMetaWire[]>(() => client.schema(), []),
          ]);
          return { client, meta: { datasets: payload.datasets, wire, jobs: payload.jobs, sources, markets, schema } };
        };

        let ready;
        try {
          ready = await load(endpoint);
        } catch (cause) {
          // 守护进程重启会换 token：清掉缓存（并宣告注入值不可信）后重新发现一次。
          const authFailure = cause instanceof DatahubError && (cause.status === 401 || cause.status === 403);
          if (!endpointOverride && authFailure) {
            resetEndpointCache();
            const rediscovered = await resolveEndpointAsync();
            endpointHolder.value = rediscovered;
            try {
              ready = await load(rediscovered);
            } catch (retryCause) {
              if (retryCause instanceof DatahubError && (retryCause.status === 401 || retryCause.status === 403)) {
                setState({ kind: 'error', message: authMessage(rediscovered) });
                return;
              }
              throw retryCause;
            }
          } else {
            throw cause;
          }
        }
        if (cancelled) return;
        setState({ kind: 'ready', ...ready });
      } catch (err) {
        if (cancelled) return;
        const endpoint = endpointHolder.value;
        if (err instanceof DatahubError && err.code === 'unreachable') {
          setState({
            kind: 'unreachable',
            endpoint: endpoint?.baseUrl ?? '',
            message: `数据服务未运行（${endpoint?.baseUrl ?? '未知端点'}）`,
          });
          return;
        }
        if (err instanceof DatahubError && (err.status === 401 || err.status === 403)) {
          setState({ kind: 'error', message: authMessage(endpoint) });
          return;
        }
        setState({ kind: 'error', message: err instanceof Error ? err.message : String(err) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [nowTs, endpointOverride]);

  useEffect(() => {
    reload();
  }, [reload]);

  return { ...state, reload };
}
