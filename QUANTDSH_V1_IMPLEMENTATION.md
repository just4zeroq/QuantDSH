# QuantDSH v1 实现方案

> **版本**：v1.0 · 2026-10-26
> **定位**：`QUANTDSH_PRODUCT_PLAN_FINAL.md`（产品方案终版 v1.4）的**实现层配套文档**。产品方案说"做什么"，本文说"怎么做、按什么顺序做、哪里留缝"。
> **上游约束**：产品方案的决策仍然有效；本文只补充实现细节，不改变任何已锁定决策。
> **本轮三条前提**（用户 2026-10-26 拍板）：
> 1. **服务端暂时不做**，先把客户端功能做完，之后再接服务端（不是取消，是延后 → 必须留接缝）
> 2. **改名 QuantDSH 全部改，不考虑兼容性**（不写数据迁移，直接断代）
> 3. **技术栈尽量 Rust**（本文 §3 给出分层依据）

---

## 0. 三条前提的实现含义

### 0.1 "暂时不做服务端" ≠ 砍掉服务端设计

这两件事在代码上的差别很大，先分清：

| 项 | v1 处理 | 理由 |
|---|---|---|
| `can_access(feature_key)` 唯一判定出口 | **必须现在就做** | 之后接服务端时只换实现，UI 与工具代码零改动 |
| 18 个 `feature_key` | **保留**，退化为功能分组标签 | 分组本身对 UI 组织有价值，且是未来的门禁键 |
| `symbol_scope`（免费额度按币种） | **保留接口**，v1 返回 `Unlimited` | 接服务端后由快照下发，不改调用方 |
| 团队 / 席位 / 订阅 / 支付 UI | **不挂载** | 半成品 UI 比没有更糟；但数据模型与路由在文档里已定稿 |
| token 登录 / 签名快照 / 设备绑定 / 实盘票据 | **不实现** | 全部依赖服务端，v1 无对应物 |
| `team.collab` / `market.publish` / `notify.alerts` | **不实现，也不在 UI 出现** | 无服务端即无法成立 |
| 服务端契约（表结构 / 路由 / token 格式） | **文档保留**（PRODUCT_PLAN_FINAL §4） | 决定"之后接得上"，改动成本为零 |

**一句话**：v1 里凡是"要靠服务端才能为真"的东西都不写代码，但凡"将来要换实现"的地方都必须先抽成接口。判断标准是——**如果接服务端时需要改调用方，说明接缝没留对**。

### 0.2 改名：全量改，直接断代

不做迁移代码，意味着凡是"读旧目录 / 旧文件名 / 旧环境变量"的分支可以**直接删除**，这是净减代码。但要小心两类"看起来像迁移、实际是功能"的东西：

- `runtime_pack` 的离线包校验（`manifest.product`）——不是兼容逻辑，是完整性校验，改名后要同步发布新包
- `runtime_compatibility` 的撤销记录——不是兼容逻辑，是状态文件，改名即可

### 0.3 Rust 优先的分层边界

见 §3.1。一句话：**能算的都在 Rust，能显示的都在 TS**。DSH 插件宿主是 Node，这一层绕不过去，但可以让它薄到只做转发。

---

## 1. 改名执行清单

### 1.1 三层归属

调查结论（`deepseek-harness` 全仓 `workdsh` **0 命中**，含装配好的 `primary-runtime`）：

| 层 | 位置 | 性质 | 是否需重建运行时 |
|---|---|---|---|
| **L1 carrier** | 本仓库 `src-tauri/` + `scripts/` + `runtime-cli/` + `package.json` | 我们自己的字符串与常量 | 不需要 |
| **L2 profile** | `D:\code\github\workdsh\apps\web`（16 个 workspace 包） | 我们 fork 后自己维护 | 不需要 |
| **L3 runtime** | `deepseek-harness`（`@deepseek-ai/dsh@0.2.1-rc.2`） | 上游，无 `workdsh` 标识 | **0 项** |

### 1.2 L1 carrier 改动表

按"定义位置"列出（不是所有引用位置，改定义即生效）：

| # | 标识 | 现值 | 新值 | 定义位置 | 备注 |
|---|---|---|---|---|---|
| 1 | 协议 scheme | `workdsh` | `quantdsh` | `src-tauri/src/app.rs:2153`（`register_uri_scheme_protocol`） | 唯一注册点 |
| 2 | Windows 兼容 host | `workdsh.localhost` | `quantdsh.localhost` | `src-tauri/src/app.rs:196` | WebView2 不能导航非标准 scheme 的绕行 host |
| 3 | 非 Windows 基址 | `workdsh://localhost` | `quantdsh://localhost` | `src-tauri/src/app.rs:198` | |
| 4 | scheme 判定 | `"workdsh"` | `"quantdsh"` | `app.rs:226 / 244 / 249 / 532 / 1457` | 白名单与命令分发 |
| 5 | 资源目录 | `workdsh-runtime` | `quantdsh-runtime` | `tauri.conf.json:39` ↔ `runtime_pack.rs:293` | **必须成对改**，一处漏了就是"运行时缺失" |
| 6 | 部署配置 | `workdsh-config.json` | `quantdsh-config.json` | `tauri.conf.json:40` ↔ `paths.rs:149` ↔ `prepare-deployment-config.mjs:34` | 三处 |
| 7 | 产品常量 | `PRODUCT = "WorkDSH"` | `"QuantDSH"` | `runtime_pack.rs:28` | 与离线包 `manifest.product` 比对（`runtime_pack.rs:523`） |
| 8 | 数据根目录 | `%APPDATA%\WorkDSH` | `%APPDATA%\QuantDSH` | `paths.rs:24`（Win）/ `:28`（mac）/ `:36`（Linux） | 三平台都要改 |
| 9 | 数据目录覆盖变量 | `WORKDSH_DESKTOP_USER_DATA` | `QUANTDSH_DESKTOP_USER_DATA` | `paths.rs:13` | |
| 10 | 其他环境变量 | `WORKDSH_DSH_HOME` 等 6 个 | `QUANTDSH_*` | `paths.rs:122`、`runtime_pack.rs:108-110`（`WORKDSH_BUNDLED_PROFILE` / `_PRIMARY_RUNTIME` / `_NODE_EXECUTABLE`）、`WORKDSH_RUNTIME_DIR`（`runtime_pack.rs:246`） | 建议一次性收进 `brand.rs` 常量 |
| 11 | profile 名 | `PROFILE_NAME = "workdsh"` | `"quantdsh"` | `paths.rs:3` | 影响 `profiles\<name>` 目录 |
| 12 | profile 目录拼接 | `home.join("profiles").join("workdsh")` | `...join(PROFILE_NAME)` | `local_runtime.rs:219`、`:591` | **改成读常量**，不要两处硬编码 |
| 13 | 版本基线状态文件 | `.workdsh-desktop-base.json` | `.quantdsh-desktop-base.json` | `local_runtime.rs:13` | |
| 14 | 发布标记文件 | `.workdsh-desktop-release.json` | `.quantdsh-desktop-release.json` | `local_runtime.rs:80` | |
| 15 | 启动器脚本 | `.workdsh-launch.mjs` / `.workdsh-host.mjs` | `.quantdsh-*` | `local_runtime.rs:561 / 592` | |
| 16 | 生成补丁的插件 id | `workdsh-identity-local` / `workdsh-desktop-identity` / `workdsh-desktop-collaboration` / `workdsh-session-access` / `workdsh-tool-access` | `quantdsh-*` | `local_runtime.rs:647-659` | 与 profile 内插件 id 必须一致，否则 patch 找不到目标 |
| 17 | 企业窗口分区 | `Partitions\workdsh-enterprise-*` | `Partitions\quantdsh-enterprise-*` | `app.rs`（enterprise 相关） | 旧 cookie 丢弃，符合"不兼容"决策 |
| 18 | 撤销记录文件 | 含 `workdsh` 的两个文件名 | `quantdsh-*` | `runtime_compatibility.rs` | |
| 19 | crate / 包名 | `workdsh-desktop-tauri` | `quantdsh-desktop-tauri` | `Cargo.toml` / `package.json:2` | |
| 20 | 脚本文件名 | `pack-workdsh-profile.mjs`、`prepare-workdsh-*.mjs`（3 个）、`workdsh-package-boundary.mjs` | `quantdsh-*` | `scripts/` | 同时改 `package.json` 里的 npm script 引用 |
| 21 | 临时目录前缀 | `workdsh-runtime-pack-*` / `workdsh-launcher-*` | `quantdsh-*` | `runtime_pack.rs:698`、`local_runtime.rs:730` | 仅美观，顺手改 |
| 22 | 文案与注释 | 残留 `WorkDSH` | `QuantDSH` | 各文件头注释、错误消息、README | 不含契约标识的注释才改 |

**实施建议（重要）**：不要直接全仓 sed。先做一步**常量收敛**——新建 `src-tauri/src/brand.rs`：

```rust
pub const PRODUCT: &str = "QuantDSH";
pub const SCHEME: &str = "quantdsh";
pub const HOST: &str = "quantdsh.localhost";
pub const PROFILE_NAME: &str = "quantdsh";
pub const RUNTIME_DIR: &str = "quantdsh-runtime";
pub const CONFIG_FILE: &str = "quantdsh-config.json";
pub const USER_DATA_DIRNAME: &str = "QuantDSH";
pub const ENV_PREFIX: &str = "QUANTDSH";
```

把所有字面量替换成引用（此时代码行为不变，可跑 `cargo test` 验证），再一次性改常量值。这样"改名"变成一次改动而不是 530 次，且以后换品牌只改一个文件。

### 1.3 L2 profile 改动表

profile 源码在 `D:\code\github\workdsh\apps\web`（261 文件 / 28.9 MB，其中 `website/` 22.6 MB 是文档站，**不要带**）。

**先决决策：profile 源码要 fork 进本仓库，不能再依赖外部目录。**
现在构建脚本用相对路径 `../workdsh/apps/web`，这意味着构建机必须有另一个仓库的工作副本——不可持续，也无法让改名生效。方案：

```
dsh-tauri/
  profile/                    ← 新增，从 workdsh/apps/web fork（排除 node_modules / website / .git）
  scripts/pack-quantdsh-profile.mjs   ← 默认源改为 ../profile
```

fork 后体积约 6 MB / 190 文件，可接受。`--source` 覆盖参数保留，方便本地调试。

| # | 标识 | 新值 | 位置 | 备注 |
|---|---|---|---|---|
| 1 | 根包名 | `quantdsh` | `profile/package.json:2` | |
| 2 | 16 个 workspace 包名 | `quantdsh-tabular` / `-contracts` / `-provider-browser-session` / `-provider-identity-local` / `-provider-runtime-isolated` / `-plugin-audit` / `-plugin-access` / `-plugin-skills` / `-plugin-experts` / `-plugin-connectors` / `-plugin-office` / `-plugin-library` / `-plugin-projects` / `-plugin-activity` / `-plugin-workbenches` / `-plugin-quality-analysis` / `-bundle` | 各 `packages/*/package.json` + 所有 import + `pnpm-workspace.yaml` | 机械但量大 |
| 3 | pnpm filter 串 | 全部 `workdsh-` → `quantdsh-` | `profile/package.json:17-18, 54-58` | 一行一个 filter 链，容易漏 |
| 4 | 存储域键 | `quantdsh_*` | 各插件的 storage 命名空间 | 与数据断代一致 |
| 5 | 工具名 | `quantdsh_*` | tool 注册处 | 出现在 preset 里，要同步 |
| 6 | 服务键 | `quantdshIdentity` / `quantdshAccess` | identity / access 插件 | |
| 7 | 插槽名 | `quantdsh.workbench.frame` | `scripts/native-workbench-frame-patch.json` + `.mjs` | **改后要重算字节补丁的两个 sha256**，否则补丁静默失效 |
| 8 | 关键帧/资源路径 | 含 `workdsh` 的静态路径 | `profile/assets`、`profile/scripts` | 逐个核对 |

**第 7 项是唯一容易"改完不报错但功能不对"的点**：`native-workbench-frame-patch.json` 记录了替换目标的 sha256，插槽名长度变了会导致偏移与哈希全部失效。做法是改完名字后重新生成该补丁，不要手改哈希。

### 1.4 执行顺序与验收

顺序不能颠倒，否则中途无法启动：

1. **常量收敛**（1.2 的 `brand.rs`）→ `cargo test` 40 项全绿，行为不变
2. **carrier 改名**（改常量值 + 成对的资源映射 + 脚本文件名）→ `npm run check:pack` 能出包
3. **profile fork + 改名**（含补丁重算）→ `npm run pack:profile` 通过
4. **全量回归**：`cargo test` + `packaged-host-smoke.mjs` + `verify:packaged` + `verify-profile-release`
5. **清空旧数据目录**（`%APPDATA%\WorkDSH` 不删也行，新进程不会再读；但测试时要确认走的是新目录）

验收标准：装了新包之后，`%APPDATA%\QuantDSH` 下生成 `profiles\quantdsh\`、`.quantdsh-desktop-base.json`、`dsh-home\`，窗口能起来，与 Profile 通信正常，旧目录**完全没有新写入**。

---

## 2. 保留的接缝：`can_access`

### 2.1 唯一判定出口

```rust
// qd-core/src/entitlement.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureKey { DataCrypto, DataCn, ResearchBacktest, /* … 18 项 */ }

pub enum SymbolScope { Unlimited, Limited(&'static [&'static str]) }

pub struct EntitlementSnapshot {
    pub source: Source,          // Local | Server
    pub org: Option<String>,     // v1 = None
    pub expires_at: Option<i64>, // v1 = None
    pub scope: SymbolScope,      // v1 = Unlimited
}

pub trait Entitlement: Send + Sync {
    fn snapshot(&self) -> EntitlementSnapshot;
    fn can_access(&self, key: FeatureKey) -> bool;
    fn scope_for(&self, key: FeatureKey) -> SymbolScope;
}
```

v1 实现：

```rust
pub struct LocalEntitlement;
impl Entitlement for LocalEntitlement {
    fn snapshot(&self) -> EntitlementSnapshot {
        EntitlementSnapshot { source: Source::Local, org: None, expires_at: None, scope: SymbolScope::Unlimited }
    }
    fn can_access(&self, _: FeatureKey) -> bool { true }
    fn scope_for(&self, _: FeatureKey) -> SymbolScope { SymbolScope::Unlimited }
}
```

### 2.2 强制性约定（写进 review checklist）

1. **任何功能入口的第一行必须是 `if !ent.can_access(key) { return Err(Denied) }`**——不是 UI 层判断，是命令层判断。将来接服务端时，这一行就是门禁。
2. **feature_key 不作为逻辑分支**。禁止出现 `if key == ResearchFactorAutoMine { 特判 }`，key 只用于判定可否。
3. **`scope_for` 必须被数据层调用**。v1 返回 `Unlimited` 看不到效果，但调用点必须存在（在 symbol 解析处），否则将来加币种限制要改数据层。
4. 服务端字段（`org` / `expires_at`）在 v1 恒为 `None`，但类型上必须存在，避免之后改结构体。

### 2.3 明确不做的事

- 不写"本地授权文件 + 校验"这种伪门禁——延迟期无意义，还会给之后挖坑（两套语义冲突）。
- 不在 UI 上显示"已解锁/需订阅"标记——v1 全解锁，标记会误导。
- 不预留"输入 license key"的输入框。

---

## 3. 引擎架构

### 3.1 进程模型

```
┌──────────────────────────────────────────────────────────────┐
│ QuantDSH.exe (Tauri carrier, Rust)                           │
│  · 窗口 / 协议 / 运行时装配 / 生命周期                        │
└───────────┬──────────────────────────┬───────────────────────┘
            │ 拉起                     │ 拉起
            ▼                          ▼
┌───────────────────────┐   ┌──────────────────────────────────┐
│ quantdsh-engine.exe   │   │ DSH host (node)                  │
│ (Rust sidecar)        │   │  · profile: quantdsh             │
│  · 数据接入 / 存储     │◄──┤  · 插件（TS）：tool 转发 + UI     │
│  · 因子 / 回测 / 仿真  │HTTP│                                  │
│  · axum 本地控制面     │   └──────────────────────────────────┘
└───────────────────────┘
```

**三个进程，不合并**，理由：

| 决策 | 理由 |
|---|---|
| 引擎独立成 sidecar，不嵌进 carrier | ① 引擎崩溃（DuckDB/网络）不能带走窗口；② 可单独重启；③ 引擎依赖重（arrow/duckdb/polars），独立 crate 编译缓存更好 |
| 用 localhost HTTP，不用 stdio | 插件与 DSH host 同进程，stdio 会抢宿主的 stdin/stdout |
| carrier 不直接调引擎 | 避免出现两套调用路径；引擎只有插件一个客户端 |

### 3.2 通道安全与生命周期

```
启动顺序：
1. carrier 生成 32 字节随机 token（内存 + 写入 userData\runtime\engine-token.json，0600）
2. carrier 选空闲端口（bind 127.0.0.1:0 取端口后释放，或引擎自己 bind 后回写端口）
3. 引擎启动，只监听 127.0.0.1，校验 Authorization: Bearer <token>
4. carrier 把 {port, token} 通过环境变量传给 DSH host → 插件读环境变量
5. 会话结束 / carrier 退出 → 杀引擎进程树
```

约定：

- 绑定地址**硬编码 `127.0.0.1`**，不监听 `0.0.0.0`。
- token 每次启动重新生成，不落盘长期保存（写文件只为跨进程传递，退出时删除）。
- 不做 CORS（非浏览器客户端），但校验 `Origin` 缺失或为本地协议。
- 引擎只接受本进程 token，无 token 返回 401 且不泄漏内部错误。

### 3.3 crate 结构

```
dsh-tauri/
  src-tauri/            ← carrier（已有）
  engine/               ← 新增 Cargo workspace
    Cargo.toml          （workspace）
    qd-core/            类型、错误、feature_key、entitlement trait
    qd-store/           DuckDB + Parquet，schema 与迁移
    qd-market/          Binance / OKX 适配，增量同步
    qd-factor/          因子表达式解析、AST 校验、计算、评估
    qd-backtest/        回测与撮合仿真
    qd-exec/            实盘/仿真执行（v1 只有仿真）
    qd-api/             axum 路由
    quantdsh-engine/    bin，装配
```

依赖选型：

| 用途 | crate | 说明 |
|---|---|---|
| 异步运行时 | `tokio`（full） | |
| HTTP 服务 | `axum` + `tower-http` | 与 Tauri 生态一致，轻 |
| HTTP 客户端 | `reqwest`（rustls） | 不用 OpenSSL，避免 Windows 打包依赖 |
| WebSocket | `tokio-tungstenite` | 行情订阅 |
| 列式存储 | `duckdb`（bundled） | 自带 C++ 库，无需系统安装；**构建慢，首次 ~6-10 min** |
| 内存列式 | `arrow` | DuckDB 与 Parquet 之间的交换格式 |
| Parquet | `parquet`（arrow 生态） | 读写落盘 |
| 序列化 | `serde` + `serde_json` | |
| SQLite（任务/元数据） | `rusqlite`（bundled） | 比 sqlx 轻，v1 无并发写需求 |
| 时间 | `chrono` + `chrono-tz` | 交易所时区 |
| 数值 | `ndarray` / `polars`（可选） | 因子批量计算先用 DuckDB SQL，polars 仅在需要 Rust 内算子时引入 |
| 日志 | `tracing` + `tracing-subscriber` | |
| 错误 | `thiserror` / `anyhow` | 库用 thiserror，bin 用 anyhow |

**关于 `polars`**：体积大（编译 15 min+，产物 +30 MB）。v1 建议**不引入**，因子计算全部走 DuckDB SQL（窗口函数足够覆盖 `ts_mean/ts_std/rank/corr`）。只有出现 DuckDB 表达不了的算子时才评估引入。

### 3.4 Tauri 打包 sidecar

```jsonc
// src-tauri/tauri.conf.json
"bundle": {
  "externalBin": ["../engine/target/release/quantdsh-engine"]
}
```

约定：

- Tauri 要求二进制名带 target triple：`quantdsh-engine-x86_64-pc-windows-msvc.exe`，由 `scripts/prepare-engine.mjs` 自动 copy+重命名。
- 引擎**不能**依赖系统运行时（纯静态 musl 不适用于 Windows；MSVC 下确保无额外 DLL 依赖）。
- 体积预算：carrier ~8 MB + 引擎 ~35-45 MB（含 DuckDB）→ 安装包约 50-60 MB（LZMA 后）。

---

## 4. 数据层

### 4.1 目录布局

```
%APPDATA%\QuantDSH\
  profiles\quantdsh\            # DSH profile（运行时契约，勿动结构）
  dsh-home\                     # DSH home
  quant\                        # ← 我们的数据根（v1 新增）
    lake\
      bronze\
        exchange=binance\market=spot\symbol=BTCUSDT\freq=1h\year=2026\*.parquet
        exchange=okx\market=swap\symbol=BTC-USDT-SWAP\freq=1m\year=2026\*.parquet
      silver\                   # 清洗对齐后（统一 UTC、去重、补齐时间轴）
        ...
    catalog.duckdb            # 视图 + 元数据 + 因子/回测结果
    jobs.sqlite               # 任务队列（因子挖掘、批量回测），可断点续跑
    logs\
```

分区键顺序固定：`exchange / market / symbol / freq / year`。这样 `read_parquet` 的通配与谓词下推都能用上。

### 4.2 DuckDB 主要表

| 表 | 用途 | 关键列 |
|---|---|---|
| `instruments` | 标的元数据 | `exchange, symbol, market, base, quote, tick_size, lot_size, listed_at, delisted_at` |
| `bars` | 视图，union 所有 bronze/silver parquet | `symbol, freq, ts, open, high, low, close, volume, quote_volume, trades` |
| `funding` | 永续资金费率 | `symbol, ts, rate` |
| `factors` | 因子定义 | `id, name, expr, vars, universe, freq, direction, provenance, created_at` |
| `factor_values` | 因子值（按 `factor_id` 分文件落 Parquet） | `factor_id, symbol, ts, value` |
| `factor_eval` | 评估结果 | `factor_id, window, ic, icir, rank_ic, turnover, coverage, corr_max, deflated_sharpe` |
| `backtests` | 回测运行记录 | `id, strategy_id, spec_json, start, end, metrics_json, created_at, engine_version` |
| `strategy_specs` | 策略定义 | `id, name, version, spec_json` |
| `sim_orders` | 仿真订单 | `id, run_id, symbol, side, qty, price, status, ts` |

**PIT（point-in-time）要求**：`instruments` 必须有 `listed_at / delisted_at`，回测取 universe 时按 ts 过滤，否则会有幸存者偏差。这是 v1 就要做的，不是优化项——后期补会推翻所有历史评估结果。

### 4.3 数据接入

| 交易所 | 品种 | 接口 | 备注 |
|---|---|---|---|
| Binance | 现货 / U 本位合约 | `GET /api/v3/klines`、`GET /fapi/v1/klines` | 单次最多 1000 根，按 `startTime` 翻页；权重限频 1200/min |
| OKX | 现货 / 永续 | `GET /api/v5/market/history-candles` | 单次最多 100 根（history）/ 300（candles），按 `after` 翻页 |

同步策略：

1. **幂等键** `(exchange, market, symbol, freq, ts)`，写入前按 `ts` 去重（Parquet 层面按文件覆盖，DuckDB 层 `DELETE` + `INSERT` 或直接重写年分区）。
2. **只补缺口**：读已有最大 `ts`，从该点往前/往后增量拉；不要每次全量重拉。
3. **限频**：令牌桶 + 遇 429 指数退避（1s→2s→4s，上限 60s），退避状态记进 `jobs.sqlite`，重启后不清零。
4. **失败不静默**：每个 symbol 的同步结果写 `sync_state` 表（`last_ts, last_error, attempts`），UI 上能查。
5. v1 不做 WebSocket 实时行情——回测与因子挖掘都是历史数据，仿真可以轮询（1-5s），省一个长连接状态机。

### 4.4 为什么 bronze/silver 两层

- **bronze**：原样落地，不做任何清洗。交易所修数据时能追溯原始值。
- **silver**：统一 UTC、去重、补齐时间轴（缺失 bar 标记 `is_gap`）、统一列类型。

因子与回测**只读 silver**。这样"数据质量"问题只在一个地方修，不会散落在每个因子实现里。

---

## 5. 策略工坊

### 5.1 策略规格（spec）

策略 = **universe + 因子组合 + 权重规则 + 调仓规则 + 风控**，用 JSON/YAML 描述，引擎解释执行：

```yaml
id: stg_momentum_001
version: 1
universe:
  exchange: binance
  market: spot
  symbols: [BTCUSDT, ETHUSDT, SOLUSDT]
  filters: { min_quote_volume_30d: 10000000 }
signal:
  kind: factor_combo
  factors:
    - { id: f_mom_20d, weight: 0.5, direction: long_high }
    - { id: f_vol_20d, weight: -0.3, direction: long_low }
  transform: zscore        # zscore | rank | winsorize_zscore
rebalance:
  freq: 1d
  at: "00:00:00Z"
  top_n: 2
  weighting: equal         # equal | inv_vol | signal_prop
costs:
  fee_bps: 5
  slippage_bps: 3
risk:
  max_position_pct: 0.5
  max_turnover_per_day: 2.0
  stop_loss_pct: 0.15
```

**为什么用"因子组合"而不是"写代码"**：v1 没有安全沙箱跑用户策略代码（DSH 的 code-runtime 是给 agent 用的，不是给用户策略用的），表达式组合能覆盖 80% 的选股/选币策略，且可静态校验、可复现、可解释。

### 5.2 因子表达式：受限 SQL 模板

**形态**：因子 `expr` 是一段**受限 SQL 表达式模板**，不是完整 SQL（没有 `SELECT`/`FROM`/`JOIN`），只允许列引用 + 窗口函数 + 标量函数 + 四则运算。

```sql
-- f_mom_20d
close / lag(close, 20) - 1

-- f_vol_20d
stddev_samp(close / lag(close, 1) - 1) OVER w20

-- f_rank_volume
rank() OVER (PARTITION BY ts ORDER BY quote_volume) / count(*) OVER (PARTITION BY ts)
```

执行时引擎把它包进固定骨架：

```sql
SELECT symbol, ts, (<expr>) AS value
FROM bars
WHERE freq = ? AND ts BETWEEN ? AND ?
WINDOW w20 AS (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 20 PRECEDING AND CURRENT ROW)
```

校验（**必须在解析阶段，不能靠执行报错**）：

| 规则 | 做法 |
|---|---|
| AST 白名单 | 解析后遍历节点，只允许 `Column / Literal / BinaryOp / FunctionCall(白名单) / WindowSpec` |
| 函数白名单 | `lag, lead, sum, avg, min, max, stddev_samp, corr, rank, row_number, count, abs, log, sign, greatest, least, coalesce, power` |
| 禁止子查询 | AST 中出现 `Subquery` 直接拒 |
| 禁止跨 symbol | 窗口 `PARTITION BY` 必须含 `symbol`；若用户没写，引擎强制补 |
| 禁止未来数据 | `lag/lead` 的偏移量必须是常量；`lead` 默认禁用（除非因子显式声明 `lookahead: true` 且用在该因子的 post-processing） |
| 长度限制 | expr ≤ 4000 字符，AST 节点 ≤ 500 |

**为什么不自研 DSL**：DuckDB 已实现窗口函数与向量化执行，自研 DSL 要么翻译成同样 SQL（多一层解析器），要么自己写算子（性能差一个量级）。受限模板是"用最小成本拿到 SQL 表达力 + 静态可校验"。

### 5.3 回测引擎

**形态**：向量化为主，事件驱动为辅。

| 阶段 | 做法 |
|---|---|
| 因子计算 | DuckDB SQL 批算，结果落 Parquet |
| 信号合成 | `transform` + 权重 → `score`，按 `ts` 横截面排序 |
| 组合构建 | `top_n` 选币 + `weighting` 分配，生成目标仓位时间序列 |
| 撮合仿真 | 事件循环（Rust），逐 bar 处理：目标仓位 → 成交价（下一 bar 开盘 + 滑点）→ 手续费 → 持仓 |
| 风控 | 每 bar 检查 `max_position_pct` / `stop_loss` / `max_turnover` |
| 绩效 | 从净值序列算指标 |

**关键约定（决定回测是否可信）**：

1. **信号在 T 收盘算，成交在 T+1 开盘**。禁止同一 bar 成交（前视偏差）。
2. **手续费双边计**：`fee_bps * 2`。
3. **滑点方向不利**：买入加、卖出减。
4. **停牌/缺 bar**：`is_gap` 的 bar 不成交，顺延到下一个有效 bar。
5. **资金约束**：不允许负现金（现货），下单量按可用资金裁剪。
6. **指标定义固定**（写进文档，避免不同 run 口径不一致）：
   - 年化收益：`(nav_end/nav_start)^(365/days) - 1`（加密 365 天）
   - Sharpe：`mean(daily_ret)/std(daily_ret)*sqrt(365)`，无风险利率默认 0，可配
   - 最大回撤：`min(nav/cummax(nav) - 1)`
   - 换手：`sum(|Δposition|)/2/avg_nav`，年化

### 5.4 仿真交易

v1 只做**本地仿真**，不接实盘（实盘需要服务端票据 + 券商适配，属 P2）：

- 复用回测撮合逻辑，但输入是轮询的实时 bar/ticker。
- 订单、持仓、账户净值写 `sim_*` 表。
- 状态机：`created → submitted → filled / rejected / cancelled`，每步落库。
- **可恢复**：进程重启后从 `sim_orders` 里 `submitted` 状态的单继续处理。

---

## 6. 自动因子挖掘

### 6.1 目标与边界

**目标**：在给定 universe 与频率下，自动产出"有增量信息"的因子候选，并给出可信的评估。

**明确不做**：不承诺"挖出能赚钱的因子"；不自动上实盘；不允许无人值守地无限跑（算力预算硬限制）。

**为什么不能靠 DSH 的 jobs**：`jobs` 是进程内、不跨重启、无重试；`schedule` 有 cron 但无执行状态机。因子挖掘一轮几十分钟，必须能断点续跑。所以**任务状态机自己写，DSH 只当启动器**。

### 6.2 闭环状态机

```
        ┌──────────────────────────────────────────────────┐
        │                                                  │
  seed ──► propose ──► validate ──► materialize ──► evaluate ──► dedupe ──► select
             ▲                          │                        │          │
             │                          ▼                        │          ▼
             │                      (Parquet)                    │      mutate ──┐
             │                                                   │               │
             └───────────────────────────────────────────────────┴───────────────┘
```

| 阶段 | 输入 | 输出 | 失败处理 |
|---|---|---|---|
| `propose` | 已有因子库 + 最近 IC 报表 + 挖掘目标（如"低相关"） | 候选 `expr` 列表（≤ 50/轮） | LLM 超时 → 用模板变异兜底 |
| `validate` | `expr` | 通过/拒绝 + 原因 | 拒绝即丢弃，记录原因供后续 prompt 参考 |
| `materialize` | 通过的 `expr` | `factors` 行 + `factor_values` parquet | 计算超时 → 标记 `timeout`，不计入预算外重试 |
| `evaluate` | 因子值 + silver 数据 | `factor_eval` 行（IC/ICIR/换手/覆盖度） | 覆盖率 < 60% 直接淘汰 |
| `dedupe` | 新因子 + 已有因子 | 保留/淘汰 | 与任一已有因子 `|corr| > 0.7` → 淘汰 |
| `select` | 评估结果 | 入因子库 / 进变异池 | `|ICIR| < 0.3` 或 IC 不稳定（分段符号翻转）→ 淘汰 |
| `mutate` | 入变异池的因子 | 新候选（参数扰动 / 算子替换） | 变异代数 ≤ 3，防止退化成随机搜索 |

### 6.3 任务持久化（`jobs.sqlite`）

```sql
CREATE TABLE mining_runs (
  id TEXT PRIMARY KEY,
  config_json TEXT NOT NULL,      -- universe/freq/window/budget
  state TEXT NOT NULL,            -- propose|validate|materialize|evaluate|dedupe|select|done|failed
  round INTEGER NOT NULL DEFAULT 0,
  budget_used INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE mining_tasks (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  kind TEXT NOT NULL,             -- propose|materialize|evaluate|...
  payload_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,   -- 幂等：同一 expr+window 只算一次
  state TEXT NOT NULL,            -- pending|running|done|failed
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  started_at INTEGER,
  finished_at INTEGER
);
```

**幂等键** = `sha256(expr + universe_hash + freq + window + data_version)`。`data_version` 是新数据落库时递增的整数——数据变了要重算，数据没变就命中缓存。

断点续跑：进程重启后扫描 `state='running'` 的任务，超过 `timeout` 的置回 `pending`（`attempts+1`），`attempts > 3` 置 `failed`。

### 6.4 防自欺（这部分比算法更重要）

| 措施 | 具体做法 | 不做的后果 |
|---|---|---|
| Walk-forward 分段 | 把窗口切成 3 段（训/验/测），只在训段做选择，报告验段与测段结果 | 全样本择优 = 过拟合，报告好看但实盘必崩 |
| 多重检验惩罚 | 用 deflated Sharpe ratio：按"尝试过的候选数 N"调整显著性阈值 | 试 1000 个因子，总有几个显著——纯随机 |
| 候选池上限 | 每轮 ≤ 50 个候选，总预算 ≤ 200 个/天 | 算力烧完，产出全是垃圾 |
| 相关性去重 | 与已有因子 `|corr| > 0.7` 淘汰 | 因子库被同一信号的变体淹没 |
| PIT universe | 按 `listed_at/delisted_at` 过滤 | 幸存者偏差，回测虚高 |
| 稳定性检查 | 分段 IC 符号必须一致，否则淘汰 | 单段表现好、跨段翻转的因子不可用 |
| 记录完整历史 | 所有候选（含被淘汰的）入库，不删 | 无法复盘"为什么这个想法失败"，也无法算真实 N |

### 6.5 LLM 的角色（重要边界）

LLM **只做两件事**：提假设（生成 `expr` 候选）、解释结果（把 IC 报表翻译成人话）。

LLM **不参与**：因子打分、去重判定、是否入选的决策。原因：这些必须是确定性的、可复现的、可审计的。同一个数据跑两次必须得到同一结论，LLM 做不到。

宿主方式：写成 `SKILL.md`（挖掘口径 + 好因子的判据 + 反面例子），agent 通过 tool 调用 `quantdsh_factor_propose` / `quantdsh_factor_evaluate`，把候选与结果读回来做下一轮对话式迭代。真正的批处理由引擎跑，agent 不参与中间计算。

---

## 7. DSH 插件层（TS 部分）

### 7.1 边界

这一层只做三件事，**不允许**有业务计算：

1. **tool 转发**：把 agent 的 tool 调用转成引擎 HTTP 调用，结果原样返回。
2. **slot 渲染**：因子库、回测报告、仿真持仓这些面板。
3. **事件桥接**：引擎的进度事件 → 会话卡片 / 通知。

### 7.2 插件结构

```
profile/packages/quantdsh-plugin-quant/
  package.json                  name: quantdsh-plugin-quant
  src/
    index.ts                    插件入口：注册 tool / slot / command
    tools/
      data-sync.ts              quantdsh_data_sync
      factor-list.ts            quantdsh_factor_list
      factor-propose.ts         quantdsh_factor_propose
      factor-evaluate.ts        quantdsh_factor_evaluate
      backtest-run.ts           quantdsh_backtest_run
      backtest-report.ts        quantdsh_backtest_report
      sim-order.ts              quantdsh_sim_order
    client.ts                   引擎 HTTP 客户端（读 env 里的 port/token）
    slots/
      FactorLibraryPanel.tsx
      BacktestReportPanel.tsx
      SimPortfolioPanel.tsx
  SKILL.md                      给 agent 的使用说明
```

### 7.3 三个必须注意的坑（官方文档里不显眼）

1. **patch 是整行替换，不是深合并**。改 `cordis.patch.yml` 里某个插件的 config，必须把整行所有字段写全，漏掉的字段会变成 undefined 而不是保留默认值。
2. **`tool-*` 插件的 `disabled` 状态不代表工具不可用**。Web profile 里那些 `tool-*` 全是 `disabled`，agent 实际能用哪些工具由 **preset** 决定。所以新增工具要改 preset（`profile/presets/*`），不是改 profile patch。
3. **SKILL.md 只给指令，不给能力**。写"可以用 DuckDB 查数据"没用——agent 没有 DuckDB；真正的能力是 tool。SKILL.md 的正确写法是"什么时候调哪个 tool、参数怎么填、结果怎么解读"。

### 7.4 tool 命名与契约

统一前缀 `quantdsh_`，入参/出参都用 JSON Schema（DSH 工具注册要求），**引擎侧的 Rust 类型是唯一事实来源**，TS 侧 schema 由 `schemars` 生成的 JSON 手抄（v1 不做自动同步，改动少）。

---

## 8. AI 功能与 DSH 扩展点对照

| AI 功能 | 来源 | DSH 承载方式 | v1 | 说明 |
|---|---|---|---|---|
| 对话驱动全流程（取数→分析→回测→报告） | QuantMind | 对话 + tool + skill | ✅ | 原生 |
| 研报 / 复盘 / 归档 | QS + Mind | 对话 + deliverables + office skill | ✅ | 原生 |
| 结构化决策留痕 | QuantDinger | `outputSchema` + session event | ✅ | 原生 |
| 量化看板 / 侧栏 / 会话卡片 | QuantStudio | slots（`main` / `sidebar.panellist` / `rightbar.session`） | ✅ | 原生 |
| 定时同步 / 每日 IC 回填 | QuantMind | `schedule`（5 字段 cron，跨重启保留） | ✅ | 无执行状态，状态自己存 |
| 人机命令 `/rebalance` `/sync` | — | `ctx.commands` | ✅ | 原生 |
| 多智能体流水线（分析→多空辩论→交易员→风险） | QuantDinger | subagent + `outputSchema` + agent-team | ⚠️ 编排自研 | agent-team 是实验特性，v1 用 subagent 串行 |
| 智能体记忆 + 反思 | QuantDinger | 无原生 memory → tool + sqlite + schedule | ⚠️ 半自研 | v1 只做"回测结果记忆" |
| 因子自主进化 | QuantMind | 引擎自研 + agent 提案 | 🔧 自研 | 见 §6 |
| 模型工场（多模型 + SHAP） | QuantMind | 需 Python 生态 | ⏳ v2 | Rust 只有 linfa 级，暂不做 |
| IM 远程指令 | QuantMind | 无 IM 通道 + 无服务端 | ❌ | 依赖服务端 |
| 舆情 / 情绪 | QuantMind | `tool-web` + 自研 RSS/实体匹配 | ⏳ 可选 | v1 不做 |

**为什么 Python 被排除**：DSH 的 Python 后端在 Windows 明确不支持，且没有任何 profile 挂载它；DuckDB 有 Rust crate（自带 C++ 库），不需要 Python 生态。所以"尽量 Rust"不是勉强，是顺势。

---

## 9. 迭代计划

### P0 · 骨架（先能跑通一条链路）

1. 常量收敛 + L1 改名全部完成，`cargo test` 全绿
2. profile fork 进 `profile/`，包名改完，能打出可运行的包
3. 引擎 sidecar：`qd-core` + `qd-api` + `qd-store`，能启动、能被插件调通（`/health`）
4. carrier 拉起引擎 + token 传递 + 退出清理
5. 插件：`quantdsh_data_sync` 一个 tool 打通（Binance 单 symbol 日线 → Parquet → 查询返回）
6. `LocalEntitlement` + 18 个 feature_key 常量 + 单测

**验收**：装好的包里，agent 说"同步 BTCUSDT 日线"，能真的拉数据、落盘、回读并答出"共 N 根，最新收盘价 X"。

### P1 · 闭环

1. Binance / OKX 完整接入（现货 + 永续，多品种，断点增量）
2. 因子：表达式校验 + 计算 + 评估（IC/ICIR/换手/相关）
3. 回测引擎 + 绩效指标 + 报告面板
4. 因子挖掘状态机（含幂等、断点续跑、防自欺全套）
5. 仿真交易（轮询 + 状态机 + 持仓面板）
6. slots 面板三个 + `/sync` `/backtest` 命令

**验收**：一条完整链路——"帮我挖 20 个动量类因子，挑与现有因子相关性最低的 3 个，用它们做个日频策略回测，给出报告"，全程 agent 驱动，引擎执行，面板可查。

### P2 · 生产化

1. 数据质量看板（缺口、异常、延迟）
2. 引擎崩溃自愈与重启恢复
3. 性能（增量计算、因子值缓存命中率）
4. 实盘接入的前置工作（券商适配骨架 + 服务端票据预留）
5. 服务端接入（此时才做：key 登录、签名快照、席位、订阅）

---

## 10. 风险与验收清单

| 风险 | 影响 | 应对 |
|---|---|---|
| DuckDB crate 构建时间（首次 6-10 min） | 开发迭代慢 | CI 缓存 `target/`；引擎拆独立 crate，改 carrier 不触发重编 |
| 安装包体积 50-60 MB | 分发成本 | 可接受；后续做增量更新 |
| 交易所限频 / 封 IP | 数据同步失败 | 令牌桶 + 退避 + 状态落库；不做高频轮询 |
| 因子挖掘算力打满 | 机器卡、用户投诉 | 硬预算（候选数/CPU 时间）+ 可中断 |
| 改名漏项（成对引用） | 启动报"运行时缺失"或配置读不到 | 常量收敛 + `verify:packaged` 全量校验 + 装包冒烟测试 |
| 补丁 sha256 失效 | 工作台框架静默不生效 | 改名后重新生成补丁，不手改哈希 |
| profile fork 与上游脱节 | 上游修复拿不到 | fork 时记录基线 commit（`upstream.json` 已有），P2 评估是否定期跟进 |

**发布前必须过的检查**：

1. `cargo test`（carrier + engine）全绿
2. `npm run check:pack` + `verify:packaged` + `verify-profile-release` 通过
3. 全新机器装包 → 首次启动 → 数据目录结构正确 → 旧目录无写入
4. 卸载重装 → 数据不丢（`%APPDATA%\QuantDSH` 保留）
5. 断网启动 → 窗口正常，引擎报"无网络"而不是崩溃

---

## 附录 A · 本轮待确认项

| # | 事项 | 本文默认处理 | 影响 |
|---|---|---|---|
| 1 | 因子表达式形态 | 受限 SQL 模板（§5.2） | 若改为自研 DSL，需多一个解析器 + 算子实现层，工期 +1 周 |
| 2 | v1 是否挂团队/订阅 UI | 不挂载，只留 `can_access` 缝（§2.3） | 若保留占位界面，需额外的"即将开放"状态设计 |
| 3 | profile fork 位置 | 本仓库 `profile/`（§1.3） | 若继续引用外部目录，改名无法生效 |
| 4 | 引擎进程形态 | 独立 sidecar（§3.1） | 若嵌入 carrier，崩溃会带走窗口 |

## 附录 B · 与产品方案的对应关系

| 本文小节 | 对应 `QUANTDSH_PRODUCT_PLAN_FINAL.md` |
|---|---|
| §1 改名 | §2 产品定位（品牌层） |
| §2 接缝 | §3.3 feature_key / §3.5 订阅模型 / §3.7 免费额度 |
| §3 引擎 | §2.2 技术栈 / §5.1 进程模型 |
| §4 数据层 | §5.4 存储三区 / §6.1 数据与底座 |
| §5 策略工坊 | §6.2 策略工坊 |
| §6 因子挖掘 | §6.2（新增项，产品方案中为"自动因子挖掘"） |
| §7 插件层 | §5.3 Profile 扩展点 |
| §8 AI 对照 | §6 功能模块全集 |
| §9 迭代 | §10 路线图 P0/P1/P2 |
