# 数据工坊与行情数据服务：运行机理与故障复盘

本文记录 `quantdsh-plugin-data` / `quantdsh-plugin-market` 两个插件页与行情服务
（`services/datahub-api`）的交付链路，以及 2026-10-27 两次「页面退回旧版本」事故的根因。
下次改这两个插件后，请先读第 5 节的排查顺序，能省掉一轮瞎猜。

> 2026-10-27 之后：行情服务已从 Rust sidecar（`src-tauri/datahub`，已删除）换成 Python
> FastAPI + ccxt（`services/datahub-api`，随安装包自带 CPython 启动）。**HTTP 契约没变**
> （`/health` 免鉴权、其余 Bearer token、`datahub.json` 字段不变），所以下面所有关于
> 客户端取数、注入快照、排查顺序的结论仍然有效；变的只有服务实现位置和 K 线表名
> （`bars` → `ohlcv`）。本文其余部分的 Rust 描述按历史记录阅读。

## 一、组成

| 部分 | 位置 | 说明 |
|---|---|---|
| 行情数据服务（sidecar） | `services/datahub-api`（旧：`src-tauri/datahub`，已删除） | Python FastAPI + ccxt + DuckDB，默认 `127.0.0.1:17821`；**除 `/health` 外全部要求 Bearer token** |
| 数据落盘 | `%LOCALAPPDATA%\QuantDSH\data` | `market.duckdb`（K 线 `ohlcv` 表，旧名 `bars`）、`meta.sqlite`（数据源/目录/任务/覆盖率）、`datahub.json`（端口 + token） |
| 进程编排 | `src-tauri/src/datahub_daemon.rs` | 随载体启动服务（`runtime/python/python.exe -s -B -m app`）；**并把 endpoint 注入窗口初始化脚本** |
| 数据工坊页面 | `profile/packages/plugins/data` | 普通插件页（非 workbench）：`sidebar.panellist` 注册 `数据工坊`，`order: -10`；页面只有 行情 / 数据 / 管理 三页 |
| 行情页面 | `profile/packages/plugins/market` | 独立产品插件 |
| 客户端构建 | `profile/apps/web/scripts/build-{data,market}.mjs` | esbuild 打包 `src/client.tsx` → `dist/client.browser.js` |

**客户端取数（token 的三级发现）**：注入的 `__QUANTDSH_DATAHUB__` → 同源路由
`GET /api/quantdsh-datahub-endpoint`（Host 侧用 `ctx.inject(['connection'])` 注册，只有 owner
peer 可达，token 不进 URL）→ 默认端口兜底。命中 401/403 时清缓存、标记注入值不可信、重试一次。

注入值是**窗口创建时的快照**：sidecar 重启会换 token，快照随即永久过期。所以真正可靠的路径是
同源路由，注入只是省一次往返。

## 二、为什么一个插件包会「被还原」

runtime 在磁盘上有**两份**副本：

| 路径 | 角色 |
|---|---|
| `build/quantdsh-runtime` | `src-tauri/tauri.conf.json` 的 `bundle.resources` 源，由 `npm run pack:runtime`（pack → prepare）生成 |
| `src-tauri/target/debug/quantdsh-runtime` | 载体实际读取的那份；`cargo build` 时由 tauri-build 按 `resources` 映射**从 build 树合拷过来**（同一个拷贝步骤也是「sidecar exe 被占用导致构建失败」的来源） |

推论：**只更新 `target/debug` 那一份，下一次 `cargo build` 必然把 build 树里的旧包盖回来。**
build 树是权威，target 只是拷贝产物。

而 build 树当时为什么一直停在旧版本：

`scripts/prepare-quantdsh-runtime.mjs` 的安装成功路径结尾是 `rmSync(backup)`（删上一版
runtime 备份）。这行写在 `try` 里，被 safe-delete / 杀软 / 锁文件拦下时抛错 → 进入 `catch`
→ 把刚装好的 `output` 整个删掉、把旧 `backup` 换回来。于是每次 `pack:runtime` 都在最后一步
白跑并整体回滚，build 树永远停在旧包。现已把备份清理移到 try 之外单独 try，失败只告警。

## 三、复盘 A：重新编译后页面退回 5 个 tab，并报 `unauthorized`

**现象**：为改启动流程执行 `cargo build` 后，数据工坊从三页退回旧的五页
（行情 / 数据管理 / SQL 查询 / 数据集目录 / 导入数据），且报 `读取失败 unauthorized`。

**根因（构建链，不是前端代码）**：当时新包只同步进了 `target/debug`（12:48），build 树仍是
11:46 的旧包（`quantdsh-plugin-data …0d0d3f77.tgz` 66541 B、`client.browser.js` 83889 B，
其中**没有** `quantdsh-datahub-endpoint` 路由）。14:4x 那次 `cargo build` 把旧包整套盖回
target，于是：

- 5 个 tab 回来了（旧 bundle）；
- 鉴权退回「只认注入快照」的老实现 → token 一过期就 `unauthorized`。

**修复**：把新包重装进**两棵树**，并补齐每棵树各自的 7 处元数据（见第 6 节）；同时修掉
prepare 的回滚触发点。校验：两棵树 `data/dist/client.browser.js` = 89603 B、
`market` = 2065643 B，`dist/index.js` 含 `quantdsh-datahub-endpoint`，
`verify-installation-anchor` 全绿（8 个产品服务、ownedClients 9、clientBatches 3）。

## 四、复盘 B：修完之后「还是 5 个 tab」

**不是新问题，是窗口比修复更早打开**。时间戳：

| 时刻 | 事件 |
|---|---|
| 15:26:41 | 用户启动应用（`<dsh-home>/.quantdsh-launch.mjs` 等启动脚本被重写的时刻） |
| 15:30:00 | 新插件包写进两棵 runtime 树 |
| 15:32 | 用户反馈「还是 5 个 tab」 |

宿主进程只在**启动时**加载插件，浏览器每次刷新都从磁盘重取 `client.browser.js`——两者不是
同一个时刻。这个不对称是整个项目里最容易被误判成「没修好」的坑。

**第二层原因：`immutable` 缓存**。客户端入口 URL 形如
`plugins/??<pkg>/client.js&rev=<12 位>`，响应头是
`cache-control: public, max-age=31536000, immutable`。只要 `rev` 不变，浏览器不会重取。
所以真正要回答的是「`rev` 会随插件内容变吗」——会，见下。

### `rev` 的计算方式（已确认，`@deepseek-ai/dsh-client-modules/lib/index.js`）

```js
function artifactRevision(baseline) {
  return framedHash("plugin-artifact", [String(baseline.mtimeMs), String(baseline.ctimeMs), String(baseline.size)])
}
function comboRevision(resources) {
  return framedHash("combo", resources.flatMap(r => [r.id, r.rev]))
}
// framedHash(domain, parts) = sha1(domain + "\0" + 逐段 "<字节长>:<内容>") 取前 12 位十六进制
```

- **单包 rev** = `sha1("plugin-artifact\0" + 各段)`，前 12 位
- **batch rev**（页面一次加载的那 6 MB）= 对整批的 `(id, rev)` 逐对再 sha1，前 12 位
- 关键结论：**rev 绑定的是文件元数据（mtime / ctime / size），不是内容哈希**

**实测对照**（本机 runtime，2026-10-27）：

| 检查项 | 结果 |
|---|---|
| 本地按上式复算 `quantdsh-plugin-data` | `3fb3b746c120` |
| 宿主下发的 entry rev | `3fb3b746c120` ✅ 一致 |
| 本地复算 batch rev（24 entry，含 data） | `c7612b5fbe2e` |
| 宿主下发的 batch rev | `c7612b5fbe2e` ✅ 一致 |
| 用 `rev=000000000000` 请求同一 combo | `404`（不会静默发旧字节） |

敏感性实验（在临时副本上做，不碰安装文件）：

| 改动方式 | rev 是否变 |
|---|---|
| 原地覆写，保持 size 与 mtime | 变（`utimes` 只能还原到毫秒，`mtimeMs` 带亚毫秒位） |
| 删除后重写（同 size、还原 mtime） | 变（Windows 上 `ctimeMs` 是**创建时间**，重建必变） |
| 只 touch mtime | 变 |

因此**不需要把 rev 绑到内容哈希上**：正常安装流程下它一定会变。补充一个细节——`data` 包的
`mtimeMs = 499162500000`（1985-10-26），`npm pack` 会把归档内 mtime 归一化成固定值，所以 rev
实际主要由 **ctime + size** 决定，而 ctime 每次解包都是新的。

唯一会失真的情形：写进**同一个目录项**、`mtime` 精确到亚毫秒都相同、`size` 也完全相同——
常规工具做不到（`utimes` 只到毫秒）。要绝对安全，插件更新一律「先挪走旧目录，再整目录解包」，
也就是第六节的做法。

**最后一层，也是最会咬人的**：宿主的 rev 是在**启动时**采样、固定在 `__DSH_BOOT__` 图里的；
页面刷新只是照抄这张图。文件换了、进程没换 → rev 不变 → 浏览器继续吃 `immutable` 的老字节。
这就是复盘 B 的完整机制，「必须重启应用」是硬约束而不是缓存 bug。

若已确认磁盘与宿主都是新版本、页面仍是旧的，才轮到清 WebView 的 HTTP 缓存：

```powershell
# 关掉应用后执行；不要删 Local Storage（那会丢 UI 状态）
$d = "$env:LOCALAPPDATA\io.quantdsh.dsh.ssh\EBWebView\Default"
foreach ($n in 'Cache','Code Cache','GPUCache','DawnGraphiteCache','DawnWebGPUCache') {
  if (Test-Path "$d\$n") { Rename-Item "$d\$n" "$n.stale-$(Get-Date -Format yyyyMMddHHmm)" }
}
```

## 五、排查顺序（按这个顺序走，别跳步）

1. **磁盘**：`build/quantdsh-runtime` 与 `src-tauri/target/debug/quantdsh-runtime` 两棵树都要看
   `profiles/quantdsh/node_modules/quantdsh-plugin-*/dist/client.browser.js` 的大小与时间。
   只对了一棵 = 下次构建会被还原。
2. **标记串**：在 bundle 里搜 `quantdsh-datahub-endpoint`（新 Host 侧）与 `数据集目录`（旧页面）。
   注意生产构建把非 ASCII 写成 `\uXXXX` **大写十六进制**，用小写去搜永远为 0，会误判成「代码没进去」。
3. **宿主实际下发**：`%TEMP%` 下的 `probe-bundle.mjs` 思路——按 `verify-installation-anchor` 的
   方式起一个临时 home 的宿主，拿 `?token=` 换 cookie，解析页面里的
   `globalThis["__DSH_BOOT__"]`，再逐个 fetch `graph.batches` 搜文案。这一步能一刀切分
   「服务端旧」还是「浏览器旧」。
   - 想只判断「宿主是否已经采到新文件」，不必搜文案：按第四节公式从 `statSync` 复算
     `quantdsh-plugin-*` 的 rev，和 `graph.entries[].rev` 比即可，一致就说明宿主采的就是当前磁盘。
4. **进程**：`Get-CimInstance Win32_Process` 看是否还有旧宿主；插件只在启动时加载，宿主不重启
   就永远发旧代码。
5. **时间线**：比对窗口启动时刻与文件写入时刻，差几分钟就是误判。
6. **缓存**：确认前三步都对，再清 WebView HTTP 缓存。

## 六、手工同步一个插件包要改的 8 处

救急时（不想跑整条 `pack:runtime`）需要逐处同步，**每棵树各一遍**：

1. 新 tgz 放进 `package-cache/`
2. 解包覆盖 `profiles/quantdsh/node_modules/<包名>`（旧目录先挪走，避免 safe-delete 拦截）
3. `profiles/quantdsh/package.json` 的 `file:` 依赖名
4. `profiles/quantdsh/profile-installation.json`（= package.json 改名
   `quantdsh-desktop-installation`，peers 取 optionalDependencies 的已安装版本）
5. `profiles/quantdsh/pnpm-lock.yaml` 的 file: 引用 + `integrity`（按 tarball 的 sha512 重算）
6. `package-cache/release-manifest.json` 的 `filename` 与 `sha256`
7. `profiles/quantdsh/.quantdsh-desktop-release.json` 的 marker（`release` 整段替换）
8. 树根 `profile-package.json`（第 3 步文件的副本）

两个必踩的坑：跨盘 `renameSync` 报 `EXDEV`（临时目录必须与产物同卷）；沙箱对
`target/debug` 下目录的重命名可能报 `EPERM`，退化为「tar 就地覆盖」即可。

正规做法仍是 `npm run pack:runtime`——它会把 build 树整体重建成正确状态，只是慢且需要网络
（`npx pnpm@11.28.5`）。

## 七、已知风险 / 待办

- **`rev` 只看文件元数据，不看内容**（机制已确认，见第四节）：正常安装必变，因此不需要内容哈希；
  要守的规矩是**不要原地覆盖 `client.browser.js`**，一律整目录替换。
- **宿主在启动时固定 rev 图**：插件更新后必须完全退出应用（含托盘）再启动。这不是缓存问题，
  文档里所有「还是旧页面」的判断都要先过这一条。
- **`prepare-quantdsh-runtime.mjs` 仍是全量重建**（约 900 MB、需联网），单包更新走手工同步更快，
  但容易漏改第 6 节里的某一处。
- **真实行情同步在本机跑不通**：`api.binance.com` / `api1` / `fapi` 全部超时，只有
  `data-api.binance.vision` 可达，而 `BinanceSource` 硬编码了主站。要接真实行情必须先给 base
  加候选或环境变量。
- **`MarketStore::upsert_bars` 是逐行插入且不带事务**，实测约 11 ms/行；导入规模要按此预算。
