# 数据工作台

QuantDSH 的数据资产页面。回答三件事：① 我有什么数据（数据集目录）② 够不够新、缺哪几天（覆盖率 + 新鲜度）③ 谁在动它（同步任务）。这是一个独立业务插件，与行情看盘页（`market` 插件）并列，两者共用本插件的 `domain` 层。

## 存储分工（已拍板并落地）

| 存储 | 放什么 | 为什么 |
|---|---|---|
| **DuckDB** | 行情 bar（`market.duckdb`） | 列存、分析型、原生 `UPDATE`/`DELETE`/`INSERT OR REPLACE`；重复与修正就是一条 SQL，不用手搓覆盖表 |
| **SQLite** | 元数据（`meta.sqlite`） | 数据集目录 / 同步任务与游标 / 覆盖率 / 质量记录 / 导入批次 / 查询历史 |

两者都是**免安装、单文件、进程内嵌入**（和 SQLite 一样的嵌入体验）。写操作全部收敛到 Host 进程独占，规避 DuckDB 跨进程多写者限制。

## 目录结构

```
src/
  domain/        纯逻辑层（零 I/O、零 node:* 依赖）—— UI 与引擎共用唯一实现
    market.ts      Market(四类) / Interval(9 种) / INTERVAL_MS
    symbol.ts      crypto 符号归一化
    freshness.ts   新鲜度分级
    coverage.ts    期望网格 / 缺口检测 / 日×小时矩阵
    contract.ts    as_of / ingested_at / closed_only 校验
    dataset.ts     DatasetRecord + summarizeDataset
  engine/        Host 端引擎（含 node:sqlite 与原生 duckdb，浏览器不可引用）
    market.ts      DuckDB 行情存储（upsert / 查询 / 修正 / 删除 / 只读 SQL）
    metadata.ts    SQLite 元数据（目录 / 任务 / 覆盖率 / 质量 / 导入 / 查询历史）
    sql-guard.ts   只读护栏（禁 DDL/DML、单语句、行数上限）
    csv-import.ts  CSV 导入 + 质量门禁 + 晋升
    index.ts       DataEngine 门面
  sources/       数据源适配层
    types.ts       DataSource 接口（加市场 = 加适配器）
    replay.ts      确定性回放源（纯 JS，浏览器可跑，离线可测）
    ccxt.ts        CCXT crypto 适配器（transport 可注入，不装 ccxt 也能测）
  sync/
    cron.ts        5 段 cron 解析 + 下次触发时间
    runner.ts      同步执行器（先写数据、后推游标）
  quality/checks.ts  缺口 / 陈旧 / OHLC 矛盾 / 重复 / 空值
  client/         React 页面（浏览器安全）
    DataWorkbench.tsx  目录 + 详情 + 覆盖率 + 同步任务
    CoverageHeatmap.tsx 日×小时热力网格（缺格留空不填色）
    demo-source.ts     用 replay 源构造真实形态数据（集成接缝）
```

## 架构纪律

1. **`domain/` 零 I/O、零 node:* 依赖**。UI 与 Agent 工具共用，口径唯一。
2. **React client 绝不 import `engine/`** —— 那里有 `node:sqlite` 与原生 duckdb，打进浏览器 bundle 必崩。共享类型一律放 `domain/`。
3. **同步两条铁律**：先写数据后推游标（最坏只是重跑，upsert 幂等）；游标 = 已入库最大 ts。
4. **SQL 唯一出口**：所有查询过 `sql-guard`（只读 + 单语句 + LIMIT 上限）再执行，被拦截也要记入查询历史。
5. **导入落 `imported` 命名空间**，与交易所 `bronze` 隔离；过质量门禁后才可显式晋升。
6. **覆盖率缺格留空**，不填 0 —— "没数据"和"数据为 0"是完全不同的事实。

## 测试

```bash
node --test tests/*.test.mjs
```

覆盖：domain 23 · sql-guard 12 · metadata 11 · market 13 · cron+sources 18 · engine 集成 15 = **92 项**。

测试装载用 esbuild 打包后**写临时文件**再 import（`tests/_load.mjs`）。不用 `data:` URL：引擎依赖原生模块，`data:` URL 模块无路径上下文，无法解析 bare specifier。

## 构建

`pnpm build`（= `tsc -p tsconfig.json && node ../../../apps/web/scripts/build-data.mjs`）。

Host 产 `dist/index.js`，客户端产 `dist/client.browser.js`（约 28 KB —— 这个体积本身就是"duckdb 没被打进浏览器包"的证明）。`@duckdb/node-api` 必须在 `dependencies` 中登记：构建脚本据此把它列为 external。

## 已知限制 / 下一步

- **Host ↔ Client 通道未打通**：页面当前用 client 侧 replay 源渲染（可在浏览器直接运行）。打通后只需替换 `buildDatasets()`，UI 与 domain 口径不变。
- `market` 插件尚未创建，其 domain 直接 `import from 'quantdsh-plugin-data/domain'` 复用。
- 依赖安装注意：本仓是 pnpm workspace，`package.json` 含 `workspace:*`，直接用 npm install 会报 `EUNSUPPORTEDPROTOCOL`。
