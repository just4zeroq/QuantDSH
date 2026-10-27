/**
 * 宿主（Node）侧的端点发现。
 *
 * 与浏览器侧 `resolveEndpoint()` 的区别：主进程能读文件，所以优先读守护进程写的
 * **端口文件**（`datahub.json`）。端口文件存在的意义正是让"第二个启动者"能复用
 * 已经在跑的实例 —— 两个进程同时写同一个 .duckdb 是 DuckDB 明确拒绝的场景。
 *
 * 顺序：环境变量 → 端口文件 → 默认端口。
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatahubEndpoint } from './client';
import { DEFAULT_DATAHUB_PORT } from './client';

export interface EndpointFile {
  port: number;
  token: string;
  pid: number;
  started_at: number;
  data_dir: string;
}

/** 与 Rust `daemon::default_data_dir()` 保持一致的定位逻辑。 */
export function defaultDataDir(): string {
  const override = process.env.QUANTDSH_DATA_DIR;
  if (override) return override;
  const localAppData = process.env.LOCALAPPDATA;
  if (localAppData) return join(localAppData, 'QuantDSH', 'data');
  const home = process.env.HOME ?? process.env.USERPROFILE ?? '.';
  return join(home, '.local', 'share', 'QuantDSH', 'data');
}

export function readEndpointFile(dir = defaultDataDir()): EndpointFile | null {
  try {
    const text = readFileSync(join(dir, 'datahub.json'), 'utf8');
    const parsed = JSON.parse(text) as Partial<EndpointFile>;
    if (typeof parsed.port !== 'number' || typeof parsed.token !== 'string') return null;
    return {
      port: parsed.port,
      token: parsed.token,
      pid: parsed.pid ?? 0,
      started_at: parsed.started_at ?? 0,
      data_dir: parsed.data_dir ?? dir,
    };
  } catch {
    // 端口文件不存在 / 损坏：守护进程可能还没起，调用方会走到"未连接"分支。
    return null;
  }
}

export function resolveHostEndpoint(): DatahubEndpoint {
  const envUrl = process.env.QUANTDSH_DATAHUB_URL;
  if (envUrl) {
    return { baseUrl: envUrl, token: process.env.QUANTDSH_DATAHUB_TOKEN ?? '' };
  }

  const file = readEndpointFile();
  if (file) {
    return { baseUrl: `http://127.0.0.1:${file.port}`, token: file.token };
  }

  return { baseUrl: `http://127.0.0.1:${DEFAULT_DATAHUB_PORT}`, token: '' };
}
