# quantdsh-plugin-market

QuantDSH 行情（看盘）工作台。

- 图表：[LuxAlgo Vela](https://github.com/LuxAlgo/Vela)（Apache-2.0，WebGL2 渲染）— 离线 `data` 通道，不接任何外部行情源。
- 数据：从 `quantdsh-datahub` 守护进程（DuckDB）按 symbol / interval 拉取 OHLCV，映射成 Vela 的 `{ time(ms), open, high, low, close, volume }` 后注入 `chart.setMarket`。
- 与「数据」工作台共用同一守护进程、同一份 HTTP 契约：页面看到的就是引擎审计的。

## 设计要点

- 每个插件包只能注册 **一个** workbench（`quantdsh.market`），故行情是独立包，不与数据工作台混在一个包里。
- 宿主侧 `src/index.ts`（`registerManaged` + `ctx.set('quantdshMarketDataHub')`）与浏览器侧 `src/client.tsx`（`registerManaged`）各自声明**完全一致**的 `WorkbenchDescriptor`（`id` / `title` / `description` / `hostCompatibility` / `capabilities`），否则注册校验会报 `declaration-mismatch`。
- Vela 默认开启署名水印（Apache-2.0 NOTICE 要求），保留即可；`dist/THIRD-PARTY-LICENSES.txt` 同时随包分发许可文本。

## 构建

```bash
pnpm --filter quantdsh-plugin-market build   # tsc 类型声明 + esbuild 打包（host ESM / browser CJS）
```
