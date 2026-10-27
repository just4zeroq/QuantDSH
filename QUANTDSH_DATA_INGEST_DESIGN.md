# 数据工坊 · 总体设计（v1）

> 目标：把「数据接入 / 数据资产」两个页签背后的数据模型、接入通道、质量门禁定死，
> 之后按此实现，不再边写边改结构。
>
> 依据：三家交易所公开 API 实测核对（2026-10）、CCXT unified 类型定义
> （`ts/src/base/types.ts`）、现有 datahub 的 `bars` / `datasets` / `imports` 结构。

> **v1.1 修订（2026-10-27）**：把「**只接 REST，不接 WebSocket**」从一句附带说明升为硬约束，
> 全文据此重排——新增 §2.5（REST 边界）、§5.2（能力探测 / 轮询调度 / 单写者），
> 数据类型砍掉 `liquidation`，每个类型增加 `mode`（可回补 / 只向前 / 采样），
> `Capabilities` 去掉 `ws` 字段，路线图删掉「WS 实时流」。改动位置见各节内联说明。
>
> **v1.2 修订（2026-10-27）**：§2.3 重写为「可获取数据清单」，改以**官方文档 + CCXT 源码**
> 交叉核对（不做接口实测）：补上 CCXT 的三家公开 REST 分组与 `has` 能力位，
> 逐类型列出端点与硬数字，并更正两处错误（OKX 其实有多空比；Binance USDⓈ-M 没有「张」的概念）。
> 结论：9 类候选里 **8 类 REST 可得**，`liquidation` 三家均无公开 REST。
>
> **v2.0 修订（2026-10-27，后端换语言）**：实施层面的三个决定，改变了本设计的落地方式，
> 但不改变已核对的取数事实（§2.3 仍然有效）：
> 1. **V1 只做 K 线**（成交价，现货 + 永续），先把「配置数据源 → 拉取 → 落库 → 目录 → 体检」跑通；
>    资金费率、持仓量、逐笔、盘口全部后移（§1.5）。
> 2. **后端由 Rust 换成 Python FastAPI + ccxt**：`src-tauri/datahub` 已删除（git 历史仍可查），
>    新实现在 `services/datahub-api`（§5 重写）。
> 3. **数据源入库**：`sources` 表（SQLite）成为唯一事实来源，新增交易所或把某家切到
>    「我们官方的聚合 API」都是改配置，不是改代码（§4.5）。
> 4. **安装包自带 Python 运行时**，用户机器不需要装 Python（§9）。
>
> 这些约束带来一个必须写进模型的区分：**venue（数据属于哪个交易所）≠ source（从哪条通道取）**。
> 详见 §4.5，这是整个设计里最容易做错的一处。

---

## 0. 结论摘要

| 决策 | 结论 |
|---|---|
| **后端** | **Python 3.12 + FastAPI**（`services/datahub-api`），Rust `quantdsh-datahub` 已删除；HTTP 契约保持 `/health` + `/api/*` + Bearer token 不变，已发布的前端不用改 |
| **取数库** | **ccxt**（仅 REST 方法，不用 `watch*`） |
| **运行时分发** | 安装包自带可重定位 CPython（`runtime/python`），启动命令 `python -m app`，`-s` 隔离用户 site-packages |
| **接入通道** | **只接 REST（HTTP 轮询），不接 WebSocket** |
| **V1 数据种类** | **只做 `ohlcv`（成交价 K 线）**，现货 + 永续两种 `inst_type`；其余类型见 §1.5 的排期 |
| **数据源** | 入库（`sources` 表）：交易所直连（binance/okx/bybit）+ **官方聚合 API** + 离线自检源；可增删改、可启停、可配镜像域 |
| **venue vs source** | `instrument_id` 只含 **venue**，`source` 仅作为 provenance 落库。同一份行情走直连或走官方 API，落同一条主键，**不会产生两份数据** |
| **字段标准** | 对齐 **CCXT unified**，交易所独有字段可空 |
| **K 线存储** | 新表 `ohlcv`，主键 `(namespace, instrument_id, interval, price_type, ts)`；旧 `bars` 一次性迁移后保留只读 |
| **命名** | `dataset_key` 6 段 `crypto:{venue}:{symbol}:{inst_type}:{data_type}:{interval}` |
| **写入** | 临时 CSV + `COPY` 到 staging，删区间后整批插入；实测 **51 万行 / 5.8 秒**（§5.4） |
| **质量门禁** | V1 六项：`gap` / `ohlc_violation` / `null_value` / `unclosed_bar` / `stale` / `price_jump` |
| **明确不做** | 逐笔全量、盘口全档、任何只能靠 WS 才拿到的数据（`liquidation` 已砍，见 §2.5） |

现有关键缺陷（本设计要修的）：

1. `bars` 表**没有 exchange 列**，`crypto:binance:BTCUSDT:1m` 与 `crypto:okx:BTCUSDT:1m`
   的 K 线会写进同一主键互相覆盖，而目录里却是两条记录。
2. `bars` 表**没有 spot/perp 区分**，`BTCUSDT` 现货与合约同 key。
3. `sync_jobs` 的 `symbols` / `interval` 是单值，表达不了「一个任务抓多标的」。
4. `quality_runs.findings` 是整块 JSON，无法按检查项查询历史。
5. `service.rs` 记质量用 `symbol:interval`，与目录键不同源，`/quality/latest` 永远查不到。

### 1.5 V1 交付范围（先跑通，再扩类型）

| 项 | V1 做 | 说明 |
|---|---|---|
| 数据类型 | `ohlcv`（成交价） | 标记价 / 指数价的主键列已留好，不实现 |
| 市场 | 现货 `spot`、永续 `swap` | 币本位（inverse）与交割合约不做 |
| 数据源 | 交易所直连、官方聚合 API、离线自检 | 由 `sources` 表驱动，可加可删 |
| 任务模式 | `backfill`（区间回补）、`incremental`（增量续拉） | 不做定时调度器，`/api/jobs/{id}/run` 显式触发 |
| 落库 | `ohlcv` + `instruments` | CSV 导入写入 `imported`，`/api/import/promote` 提升到 `bronze` |
| 质检 | 六项基础检查 | 跨周期一致性、缺口回补闭环留到 V2 |
| 前端 | 沿用现有两个插件 | 契约不变；新页面（数据接入 / 数据资产）随 V1.1 落地 |

这样切的原因：K 线是唯一「三家全覆盖 + 可按窗口回补 + 体量可控」的类型，用它把
「数据源配置 → 请求 → 规范化 → 幂等写入 → 目录 → 覆盖率 → 质检」整条链路压通之后，
再加数据类型只是加解析器和表，不会推翻任何一层。

---

## 1. 范围

**做什么**

- 交易所公开行情（**仅 REST**）：K 线、资金费率、持仓量、标的元数据；逐笔 / 盘口快照 / 多空比 / 24h 行情为可选。
- 本地文件：CSV / Parquet 导入，走同一套门禁。
- 统一的时间、精度、去重、审计、质量报告。

**不做什么**

- **不接 WebSocket**。这不是「本期不做、以后做」，而是数据层的定位决定的：
  1. 数据工坊的产出是**可重放的历史底表**，WS 事件流是「一次性、不可回放」的，断线期间的空洞
     只能靠 REST 补——那就是两套代码做同一件事；
  2. WS 的容错成本极高（订阅生命周期、序列号缺口、快照+增量对齐、重连后的补洞），
     而它换来的只是「秒级」而不是「分钟级」，对回测与质量分析没有增量价值；
  3. 多家 WS 协议各不相同，一旦引入，适配器的复杂度会从「一次请求一次响应」变成状态机，
     P0 阶段做不起。
  结论：**数据层只认 REST**；将来策略模块需要实时信号，自己去连 WS，不往这张底表里写。
- 不下单、不接私有接口、不管账户与持仓（策略模块的事）。
- 不接期权 greeks、不接链上数据。
- 不接**只有 WS 才有**的数据。已经因此被砍掉的是 `liquidation`（Binance / Bybit 无公开 REST，见 §2.5）。

---

## 2. 数据分类

### 2.1 为什么以 CCXT unified 为骨架

CCXT 的价值不是代码，是**字段共识**：100+ 家交易所的 K 线/P 线/盘口都被它压成同一套名字，
社区已经踩过各家的坑（时间单位、买卖方向、合约乘数、字符串数字）。我们的表如果和它对不上，
以后每接一家都要写一次映射。所以：**表字段按 CCXT 命名，交易所原生字段只进 `raw` JSON**。

CCXT 的核心结构（原文取自 `types.ts`）：

```
OHLCV   = [timestamp, open, high, low, close, volume]
Trade   = { id, timestamp, datetime, symbol, type, side, takerOrMaker,
            price, amount, cost, fee, order, info }
OrderBook = { symbol, timestamp, datetime, nonce, bids:[[price,amount]], asks:[[price,amount]] }
Ticker  = { symbol, timestamp, high, low, bid, bidVolume, ask, askVolume, vwap,
            open, close, last, previousClose, change, percentage, average,
            baseVolume, quoteVolume, indexPrice, markPrice, info }
FundingRate = { symbol, timestamp, fundingRate, markPrice, indexPrice,
                nextFundingTimestamp, nextFundingRate, interval, info }
OpenInterest = { symbol, timestamp, openInterestAmount, openInterestValue,
                 baseVolume, quoteVolume, info }
Market  = { id, symbol, base, quote, settle, type, spot, swap, future, contract,
            contractSize, linear, inverse, precision{tick,amount}, limits,
            taker, maker, expiry, active, info }
```

注意 `OHLCV` 只有 6 个元素——它**丢掉了**成交笔数与主动买入量。我们的表要保留这些
（见 §3.3），否则「成交量异常」「主动买卖比」这类检查做不了。

### 2.2 数据类型矩阵

> **V1 只实现 `ohlcv` 一行**（成交价，现货 + 永续）。下表保留完整的类型划分，
> 是为了让表结构一次到位（`price_type`、`mode`、`poll_interval_ms` 这些列先留好），
> 而不是 V1 就接这么多类型。优先级一列即排期。

`data_type` 枚举（存进 `datasets.data_type`）。**只收 REST 能取到的类型**，`mode` 一列是本设计
新增的硬约束——它决定这个数据集「能不能回补历史」，直接决定界面上该给「回补」还是只给「开始采集」。

| data_type | 含义 | 形态 | REST 可得性 | mode | 优先级 | 典型用途 |
|---|---|---|---|---|---|---|
| `instrument` | 标的参考数据 | 快照，非时序 | 三家都有 | `snapshot`（覆盖式） | **P0** | 合约乘数、tick size、上市时间、费率 |
| `ohlcv` | K 线 | 时序，主键 (instrument, interval, ts) | 三家都有，可按 `startTime` 深翻 | `backfill` | **P0** | 回测、指标、跨周期一致性 |
| `funding_rate` | 资金费率 | 时序，8h/4h/1h | 三家都有历史 | `backfill` | **P0** | 永续成本、基差策略 |
| `open_interest` | 持仓量 | 时序，5m–1d | 三家都有，**Binance 仅 30 天** | `backfill`（浅） | **P0** | 拥挤度、趋势确认 |
| `trades` | 逐笔 / 聚合成交 | 时序，事件流 | Binance 可按 `fromId` 回溯；**Bybit 只给最近 1000 条** | Binance `backfill`<br>Bybit `forward` | P1 | 微观结构、滑点建模 |
| `orderbook` | 盘口快照 | 时序，稀疏 | 三家只能取**当前快照**，无增量 | `snapshot`（定时采样） | P1 | 流动性、冲击成本 |
| `ticker` | 24h 行情快照 | 时序，采样 | 三家都有 | `snapshot`（定时采样） | P2 | 快速概览（不落全量） |
| `long_short_ratio` | 多空账户比 | 时序 | **三家都有**（Binance `fapiData`、OKX `rubik`、Bybit `account-ratio`） | `backfill`（浅） | P2 | 情绪指标 |
| ~~`liquidation`~~ | ~~强平~~ | — | **Binance / Bybit 无公开 REST，只有 WS** | — | **不做** | 见 §2.5 |

`mode` 的三档含义：

- `backfill`：REST 支持按时间区间翻页往回走，用户要「2024 全年 1m」就能真拉到。
- `forward`：REST 只给最近一个窗口（滚动丢弃），**只能从建任务那天开始累积**，历史补不回来。
  界面上必须显式写「只能向前采集，起始日 = 建任务日」，否则用户会以为能回补三年。
- `snapshot`：REST 只有「此刻」的状态，多次轮询得到一条稀疏序列。序列的密度 = 轮询周期，
  而不是数据本身的天然粒度。

P0 四类的理由：K 线是一切的基础；`instrument` 不给，K 线的「量」无法换算成名义额
（合约要拿 `contract_size` 才能把「张」换算成币，而三家口径不同：Binance USDⓈ-M 的
`quantity` 本来就是币，OKX SWAP 才是「张」且 1 张 = 0.01 BTC，币本位 1 张 = 100 USD）；
资金费率与持仓量是永续
策略的必要输入，而且**历史不可追溯**（Binance 的 OI 历史只留 30 天，见 §2.3），
现在不接，以后永远补不回来——注意这一条恰恰是 REST 的固有限制，不是设计取舍。

### 2.3 可获取数据清单（官方文档 + CCXT 交叉核对）

**核对方式**：以三家官方 REST 文档为准，并用 CCXT 源码交叉验证——
读 `ts/src/binance.ts`（15,968 行）、`okx.ts`（9,867 行）、`bybit.ts`（10,221 行）里
`describe().api` 的**公开**分组（排除 `ws` / `sapi` / `fapiPrivate` / `papi` 等私有组），
以及 `has` 能力位。CCXT 实现了的端点一定存在；CCXT 没实现的回到官方文档判断。
**不做接口实测**：本机对 okx/bybit 网络不可达（§9），实测取到的样本反而是噪声。

CCXT 的三家公开 REST 分组（本清单的取数范围）：

| | Binance | OKX | Bybit |
|---|---|---|---|
| 公开分组 | `public`、`fapiPublic`、`fapiData`、`dapiPublic`、`dapiData`、`fapiPublicV2` | `public` | `public` |
| 能力位要点 | `fetchOHLCV`/`fetchTrades`/`fetchOrderBook`/`fetchFundingRateHistory`/`fetchOpenInterestHistory`/`fetchLongShortRatioHistory` = true；**`fetchLiquidations` = false** | `fetchPremiumIndexOHLCV` = **false**；`fetchLongShortRatioHistory` = true（走 rubik）；`fetchLiquidations` 无 | `fetchFundingRate` = **无**（只有 `History`）；`fetchLongShortRatioHistory` = true |

#### 2.3.1 K 线（含四种价格类型）

| 价格类型 | Binance 现货 | Binance USDⓈ-M | OKX | Bybit |
|---|---|---|---|---|
| 成交价 | `public:GET klines`<br>`public:GET uiKlines` | `fapiPublic:GET klines` | `market/candles`<br>`market/history-candles` | `v5/market/kline` |
| 标记价 | — | `fapiPublic:GET markPriceKlines` | `market/mark-price-candles`<br>`history-mark-price-candles` | `v5/market/mark-price-kline` |
| 指数价 | — | `fapiPublic:GET indexPriceKlines` | `market/index-candles`<br>`history-index-candles` | `v5/market/index-price-kline` |
| 溢价指数 | — | `fapiPublic:GET premiumIndexKlines` | **无** | `v5/market/premium-index-price-kline` |

#### 2.3.2 逐笔 / 盘口 / 行情快照

| 类型 | Binance | OKX | Bybit | REST 边界（决定 mode） |
|---|---|---|---|---|
| 逐笔 | `public:trades`（近 500–1000）<br>`public:aggTrades`（`fromId` 回溯）<br>`public:historicalTrades`（**要 key**）<br>`fapiPublic:aggTrades` | `market/trades`（≤500）<br>`market/history-trades`（≤100，**仅近 3 个月**） | `v5/market/recent-trade`（≤1000，现货 ≤60，**无 id 游标**） | Binance/OKX 可 `backfill`；**Bybit 只能 `forward`** |
| 盘口 | `public:depth`（5–5000 档）<br>`fapiPublic:depth`（5–1000） | `market/books`（≤400）<br>`market/books-full`（5000） | `v5/market/orderbook`（现货 ≤200 / 合约 ≤500） | 全是 `snapshot`：**无增量，不可重建历史盘口** |
| 24h 行情 | `ticker/24hr`、`ticker/price`、`ticker/bookTicker` | `market/ticker`、`market/tickers` | `v5/market/tickers` | `snapshot` 采样 |

#### 2.3.3 衍生指标

| 类型 | Binance USDⓈ-M | OKX | Bybit |
|---|---|---|---|
| 资金费率 | `fapiPublic:fundingRate`（历史，≤1000）<br>`fapiPublic:premiumIndex`（当前 + 下次） | `public/funding-rate`（当前）<br>`public/funding-rate-history`（≤100，需 `before`/`after`） | `v5/market/funding/history`（≤200） |
| 持仓量 | `fapiPublic:openInterest`（当前）<br>`fapiData:openInterestHist`（**仅 30 天**，≤500） | `public/open-interest`（当前）<br>`rubik/stat/contracts/open-interest-history`（5m/1H/1D） | `v5/market/open-interest`（`intervalTime` 5min–1d，≤200） |
| 多空比 | `fapiData:globalLongShortAccountRatio`<br>`topLongShortAccountRatio`<br>`topLongShortPositionRatio`<br>`takerlongshortRatio`（均 30 天，≤500） | `rubik/stat/contracts/long-short-account-ratio`<br>`…-contract`<br>`…-contract-top-trader`<br>`…-position-ratio-contract-top-trader` | `v5/market/account-ratio`（5min–1d，≤500） |
| 标的参考 | `public:exchangeInfo`<br>`fapiPublic:exchangeInfo` | `public/instruments` | `v5/market/instruments-info` |

#### 2.3.4 明确拿不到的

| 类型 | 证据 | 结论 |
|---|---|---|
| 强平 / 爆仓 | CCXT 三家均**无公开 liquidation 端点**（Binance `fetchLiquidations=false`；Bybit 只有 `fetchMyLiquidations`，那是账户私有）；官方侧 Binance `/fapi/v1/forceOrders` 属 USER_DATA、需签名；Bybit 只有 WS；OKX 虽在文档中有 `/public/liquidation-orders`，但只覆盖近期窗口且不在 CCXT 模型内 | **整类不做** |
| OKX 溢价指数 K 线 | CCXT `okx.has.fetchPremiumIndexOHLCV = false`，`public` 组无对应端点 | 该类型 OKX 缺失 |
| Binance 现货资金费率 / 持仓量 | 现货无此概念，端点只存在于 `fapi*` 组 | 只能接合约 |
| Bybit 现货合约面值 / 资金费率 | 现货无合约，`v5/market/funding/history` 仅 `linear`/`inverse` | 只能接合约 |

#### 2.3.5 硬数字（决定任务预估与回补边界）

| | Binance 现货 | Binance USDⓈ-M | OKX | Bybit |
|---|---|---|---|---|
| K 线单页 | 1000 | 1500 | 300（candles）/ 100（history） | 1000 |
| K 线顺序 | 正序 | 正序 | **倒序** | **倒序** |
| 逐笔单页 | 1000（aggTrades） | 1000 | 500 / 100（history） | 1000（现货 60） |
| 历史深度 | 自上市（2017-08 BTCUSDT） | 自上市 | 自上市（`history-candles` 可回溯） | 文档未写明 → 按 `history_depth_ms` 探测 |
| 衍生指标深度 | — | **30 天**（OI 统计与多空比） | rubik 系列无明确上限 | OI/多空比可传起止，实际深度需探测 |

**必须记进设计的三条**：

1. **Binance OI 与多空比只保留约 30 天**（`fapiData:*`）。想有长历史只能自己每天抓一次落库——
   这直接决定 `open_interest` 进 P0，且必须配日更任务。
2. **OKX 的 OI 历史与多空比走 `rubik` 组**，不是 `public` 组。这两类数据在 OKX 上是
   「统计口径」而非「行情口径」，字段名与秒级行情不同（只有 `ts` + 一个比值/数量），
   映射到统一表时要单独写一套解析，不能复用 K 线那条链路。
3. **Binance 现货主域在本机不可达**（`api.binance.com` 超时），官方行情镜像
   `data-api.binance.vision` 提供**完全相同的** `/api/v3/*` 公开端点（仅公开行情，无账户接口）。
   适配器必须支持候选 base 顺序重试，且只对「连不上」重试，对 4xx 不重试
   （那是交易所给出的真实答复）。注意该镜像**只有现货** `/api/v3/*`，没有 `/fapi/*`。

#### 2.3.6 本节更正 v1.1 的两处错误

| 位置 | 原来写的 | 实际（据 CCXT + 官方文档） |
|---|---|---|
| 多空比 | 「OKX 无公开 REST」 | **OKX 有**：`rubik/stat/contracts/long-short-account-ratio` 系列，且额外提供大户持仓比、账户比两个口径。所以 `long_short_ratio` 是 **3/3 家可得**，不再是「2/3」 |
| 合约面值 | 「1 张 BTCUSDT 永续 = 0.001 BTC」 | **错**。Binance USDⓈ-M 的 `quantity` 单位就是币（1 = 1 BTC），没有「张」的概念；「张」只适用于 OKX SWAP（BTC-USDT-SWAP `ctVal=0.01`）与币本位 `BTCUSD_PERP`（`contractSize=100` USD）。所以 `contract_size` 必须**逐标的取自 `instruments`**，不能按交易所写死 |

#### 2.3.7 九类候选的最终结论

| 数据类型 | Binance 现货 | Binance 合约 | OKX | Bybit | 判定 |
|---|---|---|---|---|---|
| `instrument` | ✅ | ✅ | ✅ | ✅ | 4/4，P0 |
| `ohlcv`（成交价） | ✅ | ✅ | ✅ | ✅ | 4/4，P0 |
| `funding_rate` | — | ✅ | ✅ | ✅ | 3/3（仅合约），P0 |
| `open_interest` | — | ✅（30 天） | ✅ | ✅ | 3/3，P0 |
| `trades` | ✅ | ✅ | ✅（3 个月） | ✅（仅窗口） | 4/4，但 Bybit 只 `forward`，P1 |
| `orderbook` | ✅（快照） | ✅ | ✅ | ✅ | 4/4，仅采样，P1 |
| `ticker` | ✅ | ✅ | ✅ | ✅ | 4/4，P2 |
| `long_short_ratio` | — | ✅（30 天） | ✅ | ✅ | 3/3，P2 |
| `liquidation` | ✗ | ✗ | 部分 | ✗ | **不做** |

**一句话结论**：REST 能拿到 **8 类**（4 类 P0 全部可选），只有强平拿不到；
其中「K 线 + 资金费率 + 持仓量 + 标的」四类三家全覆盖，可以立刻开工。

### 2.4 字段可用性差异（必须允许 null）

| 字段 | Binance | OKX | Bybit | 处理 |
|---|---|---|---|---|
| K 线成交笔数 | ✅ `klines[8]` | ❌ | ❌ | 存 null，不做跨源比较 |
| 主动买入量 | ✅ `klines[9]` | ❌ | ❌ | 存 null |
| K 线是否收盘 | 由 `closeTime` 推断 | ✅ `confirm` (0/1) | 由时间推断 | 统一算 `closed` |
| 成交额（quote） | ✅ `klines[7]` | ✅ `volCcyQuote` | ✅ `turnover` | 统一 `turnover` |
| 逐笔方向 | ✅ `isBuyerMaker` 取反 | ✅ `side` | ✅ `side` | 统一 `side` = taker 方向 |
| 盘口档数上限 | 5000 | 400 | 500 | 按 source 记录 `depth` |
| 资金费率间隔 | 8h（多数）/ 4h / 1h | 8h / 4h | 8h / 4h / 1h | 存 `interval_hours` |

**设计原则**：字段缺失写 `NULL`，**不写 0**。0 会被均值、比率、异常检测当成真实观测值，
是数据质量里最难查的一类错误（详见 §5.6 的 `null_policy`）。

OKX K 线返回 9 元素：
`[ts, open, high, low, close, vol, volCcy, volCcyQuote, confirm]`，
其中 `vol` 是**张数/币数**，`volCcy` 是币数，`volCcyQuote` 才是计价货币成交额。
三者不能混用，映射时统一取 `vol` → `volume`、`volCcyQuote` → `turnover`。

Binance K 线返回 12 元素：
`[openTime, open, high, low, close, volume, closeTime, quoteAssetVolume, trades,
takerBuyBase, takerBuyQuote, ignore]`。

### 2.5 只用 REST 的边界（本节是硬约束，不是建议）

REST 是「请求—响应」模型：每次调用拿一个**区间**或一个**此刻的快照**，拿不到「变化」。
把这条推到底，会得到四个「REST 做不到」，它们已经在 §2.2 的 `mode` 列体现：

| REST 做不到 | 直接后果 | 设计上的应对 |
|---|---|---|
| 拿不到盘口的**增量**（只有全量快照） | 无法重建任意时刻的完整盘口；`orderbook` 只能是「定时采样的一张照片」 | 按 `snapshot` 模式采，采样周期写 `datasets.poll_interval_ms`（§4.3）；不承诺盘口回放 |
| 拿不到**已经滚出窗口**的逐笔 | Bybit 逐笔只能从今天开始攒；OKX 逐笔最多 3 个月 | 类型上标 `forward`，界面上写死「起始日 = 建任务日」 |
| 拿不到**只有 WS 推送**的事件 | 强平（Binance / Bybit）、Bybit 的 `allLiquidation` 全部不可得 | 直接不接，见下表 |
| 拿不到**秒级**最新状态 | 指标最新值有「轮询周期」级延迟（最小 1 分钟） | 明确 SLA：`新鲜度 ≤ 2×interval`；不承诺实时 |

被砍掉的数据类型，以及为什么不是「以后再说」：

| 类型 | Binance | OKX | Bybit | 结论 |
|---|---|---|---|---|
| `liquidation` | ✗ 公开 REST 无。`/fapi/v1/forceOrders` 是 **USER_DATA**（要签名、只能查自己账户）；公开侧只有 WS `!forceOrder@arr`，且被限到「每 symbol 每 1000ms 最多 1 笔」，本身就是**结构性低估** | ✓ `/api/v5/public/liquidation-orders`，但只给近期窗口、100 条/页 | ✗ 只有 WS `liquidation.{symbol}` | **整类不做**。只有 1/3 家能取、且窗口短到无法回测，留下来只会让「资产台账」里出现一个永远缺两家的数据集 |

由此产生三条必须写进代码的原则：

1. **轮询周期 ≠ 数据粒度**。`orderbook` 的间隔是我们选的（默认 60s），不是交易所的；
   `datasets.interval` 记交易所给的粒度，`datasets.poll_interval_ms` 记我们的采样周期
   （见 §4.3 的 ALTER），否则质量检查会拿 60s 的采样去比对 1s 的应有密度，永远报缺口。
2. **能力声明先行**。`GET /api/sources` 必须回「这家 × 这个类型 = 支持/不支持 + mode」，
   前端据此置灰选项。不允许出现「用户建了 Bybit 逐笔历史回补任务，跑完 0 行」。
3. **窗口边界要有数据**。每家每类型记一个 `history_depth_ms`（实测得出，见 §5.2.1 的探测流程），
   用户选起始时间早于它时，在抽屉里就提示「该源最早可到 2024-05-01，更早会留空」，
   而不是跑完之后在质量报告里冒出一片缺口。

反过来，只用 REST 换到三个实打实的好处，这也是选它的原因：

- **可重放**：同一条 URL 任何时候再打一次都得到同一区间，任务失败可以精确重跑；
- **可审计**：一个批次 = 一串确定的请求，`batch_items` 里能记清每一页的来源与耗时；
- **限速可控**：请求数是自己算出来的（`span / page`），能在建任务时就估出耗时，
  而 WS 的到达速率由市场决定，估不出来。

---

## 3. 统一数据模型

### 3.1 基本约定

| 约定 | 取值 | 理由 |
|---|---|---|
| 时间单位 | **毫秒 Unix，UTC** | 三家原生都是 ms；不引入秒/纳秒混用 |
| K 线 `ts` | **bar 开盘时间**，对齐到 interval 网格 | 三家一致；`closeTime` 不落库，用 `ts + interval_ms` 推 |
| 价格/数量 | `DOUBLE` | 本项目做研究与回测，不做撮合记账。若将来要精确到最小变动单位，再加 `price_ticks BIGINT`，不改现有列 |
| 符号 | 内部统一 `BTC/USDT`（CCXT 风格），落库用 `instrument_id` | 交易所原生 `BTCUSDT`/`BTC-USDT` 存 `instruments.exchange_symbol` |
| 时区 | 全库 UTC | 图表/报表在展示层转本地时区 |

### 3.2 `instrument_id`：新增的唯一标识

现有 `dataset_key` 有 4 段 `crypto:binance:BTCUSDT:1m`，**不含 spot/perp**，也不含
数据类型。新方案：

```
instrument_id = {market}:{exchange}:{symbol}:{inst_type}
例：crypto:binance:BTC/USDT:spot
    crypto:binance:BTC/USDT:swap
    crypto:okx:BTC/USDT:swap

dataset_key  = {instrument_id}:{data_type}:{interval}
例：crypto:binance:BTC/USDT:spot:ohlcv:1m
    crypto:binance:BTC/USDT:swap:funding_rate:8h
    crypto:binance:BTC/USDT:swap:instrument:*      ← 参考数据无周期，用 *
```

兼容：解析器先按 6 段尝试，失败再按旧 4 段解析并补默认值（`inst_type=spot`,
`data_type=ohlcv`）。迁移脚本把现有 8 条 `crypto:binance:BTCUSDT:*` 重写为
`crypto:binance:BTC/USDT:spot:ohlcv:*`，`instruments` 里补一条对应记录。

价格类型（§2.3 的成交价 / 标记价 / 指数价 / 溢价指数）不占段位，而是挂在 `interval` 段上，
缺省即成交价：

```
crypto:binance:BTC/USDT:swap:ohlcv:1m         ← 等价于 1m.trade
crypto:binance:BTC/USDT:swap:ohlcv:1m.mark
crypto:binance:BTC/USDT:swap:ohlcv:1h.index
```

这样 `dataset_key` 保持 6 段，而 `ohlcv.price_type` 与之 1:1 对应（解析时拆分）。
P0 只接成交价，`.mark` / `.index` 等 P2 再开——但**列和主键先留好**，否则将来加进来
就要改表 + 重写全部 key。

### 3.3 K 线的字段增强

在 CCXT `OHLCV` 之上加 4 列，都有明确用途：

| 列 | 来源 | 用途 |
|---|---|---|
| `turnover` | 三家都有 | 成交额，跨标的可比（BTC 与 DOGE 的「量」不可比） |
| `trades_count` | 仅 Binance | 平均单笔规模 = volume / trades_count，异常检测用 |
| `taker_buy_volume` | 仅 Binance | 主动买卖比，是资金流向的原始信号 |
| `vwap` | 派生 `turnover / volume` | 避免每张图重算；委托给写入时算一次 |

`closed` 必须准。Binance 的最后一根 K 线是**未收盘**的实时 bar，直接落库会让
「最新一根」在每次同步后反复变化，污染增量和指标。规则：
`closed = (ts + interval_ms) <= now - 500ms`（留半秒时钟偏差）。

---

## 4. 存储设计

DuckDB 存时序，SQLite 存元数据——沿用现状，不引入第二套引擎。

### 4.1 参考数据

```sql
CREATE TABLE IF NOT EXISTS instruments (
  instrument_id   VARCHAR PRIMARY KEY,   -- crypto:binance:BTC/USDT:swap
  market          VARCHAR NOT NULL,      -- CRYPTO | A_SHARE | US | HK
  exchange        VARCHAR NOT NULL,
  symbol          VARCHAR NOT NULL,      -- BTC/USDT（统一）
  exchange_symbol VARCHAR NOT NULL,      -- BTCUSDT / BTC-USDT / BTC-USDT-SWAP
  base            VARCHAR,
  quote           VARCHAR,
  settle          VARCHAR,               -- USDT / USDC / null(现货)
  inst_type       VARCHAR NOT NULL,      -- spot|swap|future|option|stock|etf
  contract_size   DOUBLE DEFAULT 1,      -- 合约面值，现货为 1
  tick_size       DOUBLE,
  lot_size        DOUBLE,
  min_notional    DOUBLE,
  maker_fee       DOUBLE,
  taker_fee       DOUBLE,
  listed_at       BIGINT,
  delisted_at     BIGINT,
  active          BOOLEAN NOT NULL DEFAULT TRUE,
  raw             JSON,                  -- 交易所原始 exchangeInfo 片段
  updated_at      BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_inst_ex ON instruments(exchange, inst_type);
```

`contract_size` 是关键：不存它，OKX SWAP 的「张数」永远换不成 BTC 数量
（BTC-USDT-SWAP 1 张 = 0.01 BTC，币本位 BTCUSD_PERP 1 张 = 100 USD）。
反过来说，**不能假设每家都有「张」**：Binance USDⓈ-M 的 `quantity` 单位就是币，
`contract_size` 恒为 1；把它统一成「币」的写法是 `volume_base = volume × contract_size`，
Binance 乘 1 也成立，所以这一列必须逐标的从 `instruments` 取，不能按交易所写死（见 §2.3.6）。

### 4.2 时序表

```sql
-- K 线（替代现有 bars）
-- price_type：trade（成交价，默认）| mark | index | premium
--   §2.3 里这三类价格各自都有 REST 端点，若主键不含它，「标记价 1m」会把「成交价 1m」覆盖掉
CREATE TABLE IF NOT EXISTS ohlcv (
  instrument_id VARCHAR NOT NULL,
  interval      VARCHAR NOT NULL,
  price_type    VARCHAR NOT NULL DEFAULT 'trade',
  ts            BIGINT  NOT NULL,
  open   DOUBLE, high DOUBLE, low DOUBLE, close DOUBLE,
  volume   DOUBLE,      -- base 数量（现货=币；合约=张数×contract_size 后的等价币数）
  turnover DOUBLE,      -- 计价货币成交额
  trades_count     BIGINT,   -- 可空
  taker_buy_volume DOUBLE,   -- 可空
  vwap     DOUBLE,
  closed   BOOLEAN NOT NULL DEFAULT TRUE,
  source   VARCHAR NOT NULL,           -- binance-rest / okx-rest / csv
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  batch_id VARCHAR,
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, interval, price_type, ts)
);
CREATE INDEX IF NOT EXISTS idx_ohlcv_iv ON ohlcv(interval, ts);

-- 逐笔（P1）
CREATE TABLE IF NOT EXISTS trades (
  instrument_id VARCHAR NOT NULL,
  ts        BIGINT  NOT NULL,
  trade_id  VARCHAR NOT NULL,   -- 交易所 id；聚合成交记为 'agg:<id>'
  price     DOUBLE NOT NULL,
  amount    DOUBLE NOT NULL,
  cost      DOUBLE,
  side      VARCHAR,            -- buy | sell（taker 方向）
  taker_or_maker VARCHAR,
  is_agg    BOOLEAN DEFAULT FALSE,
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  batch_id  VARCHAR,
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, ts, trade_id)
);

-- 盘口快照（P1，**只做定时采样**：REST 无增量接口，拿不到档位变化，因此不可重建历史盘口）
CREATE TABLE IF NOT EXISTS orderbook_snapshots (
  instrument_id VARCHAR NOT NULL,
  ts      BIGINT NOT NULL,
  nonce   BIGINT,
  bids    LIST<STRUCT(price DOUBLE, amount DOUBLE)>,
  asks    LIST<STRUCT(price DOUBLE, amount DOUBLE)>,
  depth   INTEGER,     -- 请求档数
  spread  DOUBLE,      -- 最优价差（绝对）
  mid     DOUBLE,      -- (bid1+ask1)/2
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, ts)
);

-- 资金费率（P0）
CREATE TABLE IF NOT EXISTS funding_rates (
  instrument_id VARCHAR NOT NULL,
  ts            BIGINT NOT NULL,   -- 结算时间
  funding_rate  DOUBLE NOT NULL,
  mark_price    DOUBLE,
  index_price   DOUBLE,
  next_funding_ts BIGINT,
  interval_hours  INTEGER,
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  batch_id VARCHAR,
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, ts)
);

-- 持仓量（P0）
CREATE TABLE IF NOT EXISTS open_interest (
  instrument_id VARCHAR NOT NULL,
  ts            BIGINT NOT NULL,
  oi_base       DOUBLE,   -- 折算成 base 币数量
  oi_value      DOUBLE,   -- 折算计价货币
  native_interval VARCHAR, -- 数据源粒度：5m/15m/1h/1d
  namespace VARCHAR NOT NULL DEFAULT 'bronze',
  batch_id VARCHAR,
  ingested_at BIGINT NOT NULL,
  PRIMARY KEY (namespace, instrument_id, ts)
);
```

**为什么盘口存 `LIST<STRUCT>` 而不是一行一档**：一次快照的所有档位必须**同一时刻**，
拆成多行后按 `ts` 聚合会放大 100 倍行数（20 档 → 20 行/快照），而查询 99% 是
「某一刻的深度」而不是「某一档的历史」。需要档位级分析时用视图展开：

```sql
CREATE VIEW IF NOT EXISTS orderbook_levels AS
SELECT s.instrument_id, s.ts, s.side, t.lvl.price, t.lvl.amount
FROM (
  SELECT instrument_id, ts, 'bid' AS side, bids AS levels FROM orderbook_snapshots
  UNION ALL
  SELECT instrument_id, ts, 'ask' AS side, asks AS levels FROM orderbook_snapshots
) s, UNNEST(s.levels) AS t(lvl);
```

### 4.3 元数据（SQLite）改造

```sql
-- 目录：加类型与标的
ALTER TABLE datasets ADD COLUMN data_type TEXT NOT NULL DEFAULT 'ohlcv';
ALTER TABLE datasets ADD COLUMN instrument_id TEXT;
-- 采集形态：backfill | forward | snapshot（见 §2.2）；snapshot 类型必须记采样周期，
-- 否则质量检查会拿「60s 采样」去比「1s 应有密度」，永远报缺口
ALTER TABLE datasets ADD COLUMN mode TEXT NOT NULL DEFAULT 'backfill';
ALTER TABLE datasets ADD COLUMN poll_interval_ms INTEGER;

-- 批次审计：一批可含多数据集，替代 imports
CREATE TABLE IF NOT EXISTS import_batches (
  id            TEXT PRIMARY KEY,
  source        TEXT NOT NULL,      -- binance | okx | bybit | csv
  channel       TEXT NOT NULL,      -- rest | file | ccxt   （没有 ws：数据层只接 REST）
  kind          TEXT NOT NULL,      -- backfill | incremental | manual
  status        TEXT NOT NULL,      -- running | ok | partial | rejected | failed
  requested_at  INTEGER NOT NULL,
  finished_at   INTEGER,
  rows_fetched  INTEGER NOT NULL DEFAULT 0,
  rows_written  INTEGER NOT NULL DEFAULT 0,
  rows_rejected INTEGER NOT NULL DEFAULT 0,
  reject_detail TEXT,               -- JSON: [{code, count, sample}]
  gate_passed   INTEGER,
  gate_detail   TEXT,
  job_id        TEXT
);
CREATE TABLE IF NOT EXISTS batch_items (
  batch_id     TEXT NOT NULL,
  dataset_key  TEXT NOT NULL,
  from_ts      INTEGER,
  to_ts        INTEGER,
  rows_written INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (batch_id, dataset_key)
);

-- 质量发现：一项一行，可按 code 查历史
CREATE TABLE IF NOT EXISTS quality_findings (
  run_id      TEXT NOT NULL,
  dataset_key TEXT NOT NULL,
  code        TEXT NOT NULL,      -- gap | stale | ohlc_violation | ...
  severity    TEXT NOT NULL,      -- ok | warn | error
  scope_from  INTEGER,
  scope_to    INTEGER,
  item_count  INTEGER NOT NULL DEFAULT 0,
  detail      TEXT,               -- JSON，缺口明细等
  detected_at INTEGER NOT NULL,
  PRIMARY KEY (run_id, dataset_key, code)
);
CREATE INDEX IF NOT EXISTS idx_qf_code ON quality_findings(dataset_key, code, detected_at DESC);

-- 任务：支持多标的、多类型
ALTER TABLE sync_jobs ADD COLUMN data_types TEXT NOT NULL DEFAULT '["ohlcv"]';
ALTER TABLE sync_jobs ADD COLUMN instruments TEXT;   -- JSON 数组；空表示按筛选条件动态解析
-- REST 轮询周期：仅增量与 snapshot 类型用；NULL 表示一次性任务
ALTER TABLE sync_jobs ADD COLUMN poll_interval_ms INTEGER;
-- 该源该类型实测可回溯的深度（ms），用于在抽屉里提前警告「更早会留空」
ALTER TABLE sync_jobs ADD COLUMN history_depth_ms INTEGER;
```

`imports` 旧表保留只读，新代码只写 `import_batches` + `batch_items`。

### 4.4 容量估算（决定「接什么、采多密」）

| 数据 | 单标的量级 | 100 标的/年 | 结论 |
|---|---|---|---|
| 1m K 线 | 525,600 行/年 | 5,256 万行 ≈ 4 GB | 可接，主力 |
| 逐笔 | 200–500 万笔/天（BTCUSDT） | 十亿级 ≈ 数百 GB | **默认不接**；按标的按区间按需拉，用完可删 |
| 盘口 | 1 秒 1 次 × 20 档 = 691 万档/天 | 不可行 | **只做稀疏采样**：1 分钟 1 次、20 档 ≈ 3.4 GB/年/标的 |
| 资金费率 | 1,095 行/年（8h） | 11 万行 | 可接，极小 |
| 持仓量 | 105,120 行/年（5m） | 1,051 万行 | 可接；5m 足够，不必 1m |

除了磁盘，**REST 还要算请求数**——这是 WS 不需要、而我们必须要付的成本。按 §2.3 的单页上限算
「一年 1m」的回补成本：

| 源 | 单页 | 一年 1m 请求数 | 限速 | 理论最短耗时 | 结论 |
|---|---|---|---|---|---|
| Binance | 1000 | 526 次（weight 5） | 6000 weight/min | **≈ 30 秒** | 回补完全可行 |
| OKX | 300（history 100） | 1,752 次 | 20 次/2s（history） | **≈ 3 分钟** | 可行 |
| Bybit | 1000 | 526 次 | 600 次/5s | **≈ 5 秒** | 可行，但受历史深度限制 |

逐笔是最容易被低估的一项：Binance BTCUSDT 一天约 300 万条聚合成交，按 1000 条/页 = **3000 次请求**，
即便限速允许，单标的回补一年也是「百万次请求 + 数十 GB」。所以结论不是「能不能拉」，
而是「值不值」——`trades` 仍然是默认关闭、按需按区间拉。

结论写进默认配置：`trades` 与 `orderbook` 的新建任务**默认关闭**，需要时按标的单独开，
并在任务上明确「保留期」。盘口因无 REST 增量，采样周期同时决定「能回答什么问题」，
默认 60s：低于 60s 会被限速卡住，高于 60s 基本只能看流动性量级。

---

## 4.5 数据源入库（`sources` 表）

### 4.5.1 先分清两个词：venue 与 source

| 概念 | 含义 | 出现在哪 | 例子 |
|---|---|---|---|
| **venue** | 数据**属于**哪个交易所 | `instrument_id`、`dataset_key`、任务表的一列 | `binance` / `okx` / `bybit` |
| **source** | 数据**从哪条通道取**，只是 provenance | `ohlcv.source`、批次审计、任务表的 `source` 列 | `binance:ccxt` / `official:official` / `csv` |

为什么必须分开：用户要的「官方 API」不是第五家交易所，而是**同一个币安行情的另一条通道**。
如果把它当成一个独立 venue，那么「币安直连的 BTCUSDT 1m」和「官方 API 给的 BTCUSDT 1m」
会变成两个 dataset、两份数据、两次体检，切一次通道就重复一遍。用 venue 做键之后：

- 换通道只是换 `sources` 里的一行配置，`dataset_key` 不变；
- 两个通道的数据**落同一条主键**，后跑的覆盖先跑的（`ohlcv.source` 保留最后一次的来源）；
- 质量报告、覆盖率、K 线图都不需要知道数据是谁取的。

这条约束在代码里只有一处：`instrument_id` 的构造函数只接受 venue。providers 负责把
「这家交易所的原始符号」映射成统一 `BTC/USDT`，但不参与 identity。

### 4.5.2 表结构（SQLite，`meta.sqlite`）

```sql
CREATE TABLE IF NOT EXISTS sources (
  id               TEXT PRIMARY KEY,      -- binance / okx / bybit / official / replay
  label            TEXT NOT NULL,
  provider         TEXT NOT NULL,         -- ccxt | official | replay  ← 代码里的实现名
  venue            TEXT,                  -- 交易所身份；聚合源为 NULL，由任务指定
  kind             TEXT NOT NULL,         -- exchange | aggregate | local
  base_urls        TEXT NOT NULL,         -- JSON 数组，按序尝试
  enabled          INTEGER NOT NULL DEFAULT 1,
  priority         INTEGER NOT NULL DEFAULT 100,   -- 同一 instrument 多通道时的默认优选
  inst_types       TEXT NOT NULL,         -- JSON：["spot","swap"]
  intervals        TEXT,                  -- JSON；NULL = 由 provider 决定
  auth_kind        TEXT NOT NULL DEFAULT 'none',   -- none | bearer | header
  auth_secret_ref  TEXT,                  -- **环境变量名**，不存明文密钥
  config           TEXT NOT NULL DEFAULT '{}',     -- JSON：url_overrides / proxy / timeout
  capabilities     TEXT,                  -- 探测结果缓存（探测才写，不猜）
  capabilities_at  INTEGER,
  status           TEXT NOT NULL DEFAULT 'unknown',-- ok | unreachable | unknown
  status_at        INTEGER,
  notes            TEXT NOT NULL DEFAULT '[]',     -- JSON：探测结论、已切镜像等
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
```

四条设计约束：

1. **`provider` 是枚举，`id` 是实例名。** 同一个 provider 可以有多条记录（币安主域、币安镜像、
   币安经官方 API），配置不同而已。新增一个镜像不需要发版。
2. **密钥只存引用名。** `auth_secret_ref` 是环境变量名，值在进程环境里读。数据库会被备份、
   会被导出、会出现在用户的诊断包里，明文 token 一旦写进去就等于泄露。
3. **`capabilities` 是缓存不是声明。** 由 `POST /api/sources/:id/probe` 真实探测后写入
   （历史深度用二分），界面只信这一列。手填的「支持 1m」在交易所改限速后就是错的。
4. **`enabled=0` 的行保留。** 官方 API 在配置地址前是禁用状态；删掉它会让「为什么没有这个
   选项」变成一个查不到原因的问题。

种子（`INSERT OR IGNORE`，用户改过的不覆盖）：

| id | provider | venue | 说明 |
|---|---|---|---|
| `binance` | ccxt | binance | `url_overrides` 依次尝试 `api.binance.com` → `data-api.binance.vision` |
| `okx` | ccxt | okx | 现货与永续同一域名 |
| `bybit` | ccxt | bybit | |
| `official` | official | — | 官方聚合 API，**默认禁用**，填了 `base_urls` 再开 |
| `replay` | replay | replay | 离线自检，确定性造数，用于跑通链路与回归 |

### 4.5.3 官方聚合 API 的协议（我们这一侧要实现的对外契约）

官方 API 的价值是「已经规范化」：客户端不做符号映射、不做分页方向判断、不做合约乘数换算。
所以它必须直接给出统一结构，而不是转发交易所原始 JSON。

```http
GET {base}/api/v1/venues
→ {"venues":[{"venue":"binance","label":"币安",
              "instTypes":["spot","swap"],
              "intervals":["1m","5m","15m","30m","1h","4h","1d"],
              "maxPage":1000,"historyDepthMs":null}]}

GET {base}/api/v1/instruments?venue=binance&instType=swap
→ {"instruments":[{"symbol":"BTC/USDT","venueSymbol":"BTCUSDT",
                   "base":"BTC","quote":"USDT","settle":"USDT","contractSize":1}]}

GET {base}/api/v1/ohlcv?venue=binance&symbol=BTC/USDT&instType=swap
        &interval=1m&from=<ms>&to=<ms>&limit=1000
→ {"bars":[{"ts":..., "open":..., "high":..., "low":..., "close":...,
            "volume":..., "turnover":..., "tradesCount":..., "closed":true}],
   "next": <ms|null>}
```

三条硬约定：

- `volume` 必须是 **base 数量**（币），不是张数。换算的责任在采集端，因为只有它知道
  `contractSize`。
- `next` 在最后一页必须是 `null`。让客户端自己判断「这页不满就是最后一页」会在
  交易所返回空档时提前截断。
- `ts` 是 bar 开盘时间，毫秒 UTC，且**已对齐网格**。

鉴权：`Authorization: Bearer <token>`，token 来自 `auth_secret_ref` 指向的环境变量。

### 4.5.4 一个容易漏的点：ccxt 的 `fetchMarkets` 必须收窄

币安的 `loadMarkets()` 默认会同时拉现货、USDⓈ-M、币本位三套。任一套不可达，整次任务就失败——
本机就是这种情况（`api.binance.com` 超时、`fapi.binance.com` 也不可达）。所以 provider 必须
按 `inst_type` 设置 `options.fetchMarkets = ['spot'] / ['swap']`，否则「只取现货」会莫名其妙
因为合约域名不通而失败。

---

## 5. 接入管线

### 5.1 适配器接口（Python）

实现落在 `services/datahub-api`：

```
app/
  config.py     # 数据目录、端口、token、端口文件（与 carrier 的契约）
  domain.py     # interval / instrument_id / dataset_key / Bar / Capabilities
  db.py         # DuckDB：instruments + ohlcv、COPY 批量写、bars 迁移
  meta.py       # SQLite：sources / datasets / sync_jobs / batches / coverage / quality
  providers/
    base.py         # Provider 抽象 + OhlcvRequest + SourceConfig
    ccxt_source.py  # 交易所直连（ccxt，只用 REST）
    official.py     # 官方聚合 API
    replay.py       # 离线自检源
  registry.py   # sources 行 → provider 实例（缓存，因为 ccxt 持有 markets 与限速器）
  sync.py       # 任务执行：分页、写库、目录、覆盖、体检
  quality.py    # 六项基础检查
  api.py        # FastAPI 路由，契约与旧 Rust 服务一致
  __main__.py   # python -m app --data-dir ...
```

```python
class Provider(ABC):
    kind: str                       # ccxt | official | replay

    async def capabilities(self, inst_type) -> Capabilities: ...
    async def instruments(self, inst_type) -> list[dict]: ...
    async def fetch_ohlcv(self, req: OhlcvRequest) -> list[Bar]: ...
    async def aclose(self) -> None: ...

@dataclass
class Capabilities:
    intervals: list[str]            # 该 venue 真正支持的粒度（来自 ex.timeframes）
    max_page: int                   # 单页上限（binance 1000 / okx 300 / bybit 1000）
    order: str                      # asc | desc，决定翻页方向
    history_depth_ms: int | None    # None = 未知，探测后回填
    min_poll_interval_ms: int
    notes: list[str]                # 「已切换到备用域名」这类事实
```

关键约定：

- **provider 只负责三件事**：符号映射（`BTCUSDT` ↔ `BTC/USDT` ↔ `BTC/USDT:USDT`）、
  时间对齐与排序归一（一律正序）、`closed` 判定。sync 引擎里**没有** `if venue == "binance"`。
- **不支持的组合抛 `Unsupported`，不返回空列表**。空列表会被 `gap` 检查读成「这段真的没数据」，
  最后变成一条永远修不好的质量告警。
- **ccxt 只调同步 API，并推到线程池执行**（`anyio.to_thread.run_sync`）。ccxt 的 async 客户端
  把 session 绑在事件循环上，在会被重启的长驻进程里只会带来麻烦。
- **`turnover` 允许为 NULL**。ccxt 的统一 `fetchOHLCV` 只给 6 个字段（丢了 quote volume 与
  成交笔数），所以我们不伪造 `volume × close`——那是「猜出来的成交额」，比缺失更危险。
- **合约量必须乘 `contractSize`**：`volume_base = volume × contract_size`（现货为 1）。
  OKX SWAP 的 `vol` 是张数、Binance USDⓈ-M 的 `quantity` 本来就是币，不乘就会差两个数量级。
- **`fetchMarkets` 按 `inst_type` 收窄**（§4.5.4），否则现货任务会被合约域名拖死。

三条通道的分工：

| provider | 数据来源 | 什么时候用 |
|---|---|---|
| `ccxt` | 交易所公开 REST | 默认；能直连就走它，延迟最低、字段最全 |
| `official` | 我们自己的聚合 API | 本机/内网无法直连交易所时；或需要统一限速与缓存 |
| `replay` | 本地确定性生成 | 离线演示、CI 回归——它走的是真实的写库/体检代码路径，不是 mock |

### 5.2 只用 REST 带来的三个专门机制

#### 5.2.1 能力探测（建任务前跑，不靠猜）

REST 的深度上限各家常变（Binance OI 从 30 天改过好几次），**不能写死在代码里**。
每家每类型做一次二分探测，结果缓存 7 天：

```
probe(source, type):
  # 1) 先要最近的，确认能取到
  # 2) 用 t0 = now - 1y 请求一次：
  #    非空 → 继续往前翻；空 → 二分 t0
  # 3) 记录 history_depth_ms = now - 最早非空 ts
  # 4) 顺带记 min_poll_interval_ms（受限速约束）
```

界面用 `history_depth_ms` 在做「建任务」时就提示边界，而不是跑完再报缺口。
`GET /api/sources/:id/probe` 就是这个探针的入口，结果写进 `sources.notes[]`。

#### 5.2.2 轮询调度器（替代 WS 的那部分职责）

没有 WS，就没有「到点推送」，只有「到点去看」。调度器规则：

| 项 | 规则 | 理由 |
|---|---|---|
| 触发时刻 | `ts_next = ceil(now / interval) * interval + delay`，`delay = 3s` | bar 收盘后 3 秒再去取，避开交易所写入抖动；取早了会拿到未收盘 bar |
| 抖动 | 每任务 ±0~5s 随机 | 20 个任务同在整分触发会瞬间打满限速 |
| 并发 | 全局并发上限（默认 8），按 host 分组 | Binance 权重是**按 IP 跨接口共享**的，并发高了会互相抢额度 |
| 跳过 | 上一批还没跑完 → 本次跳过并记 `skipped`，不排队 | 排队会让错过的批次在恢复后集中爆发，反而打爆限速 |
| 补跑 | 落后超过 `3 × interval` 时，切到 `backfill` 小段补齐，而不是把每次轮询都放大 | 避免「一个 1m 任务补 6 小时 = 360 次请求」把别的任务饿死 |
| 停机恢复 | 启动时算 `[last_closed_ts, now]`，超过阈值才回补 | 笔记本合盖一夜是常态 |

#### 5.2.3 单写者约束（REST 之外的老问题，这里一并定死）

DuckDB 单文件只允许一个写进程。调度器与手工导入、体检任务是**同一进程内的不同并发**，
必须串到一条写队列上；外部工具（含用户的 duckdb CLI）一律 `-readonly`。
这条不是 REST 带来的，但轮询调度器一上线就会暴露它。

### 5.3 分页、限速、重试

| | Binance | OKX | Bybit |
|---|---|---|---|
| 单页上限 | 1000 | 300（candles）/ 100（history） | 1000 |
| 翻页方式 | `startTime` 递增 | `after`（倒序游标）/ `before` | `start`/`end` 递减 |
| 限速（量级） | 现货 6000 weight/min；klines limit≤100 → weight 1，≤500 → 2，≤1000 → 5 | 40 次/2s（candles）、20 次/2s（history） | 600 次/5s（market 组） |
| 超限信号 | HTTP 429 / 418 | `code=50011` | `retCode=10006` |

统一策略：

- 令牌桶，**按 weight 而非按次数**（Binance 是权重制，按次数算会超）。
- 429/418/10006 时读响应头的重置时间，退避到该时刻，不盲目指数退避。
- 重试 3 次，指数退避 + 抖动；仍然失败则**批次标记 `partial`**，保留已写数据与游标。
- 绝不在重试时改动请求区间——那会造成缺口被「跳过」而不是「失败」，是最危险的一类静默错误。

### 5.4 回补与增量（按 `mode` 分流）

| mode | 模式 | 起止 | 游标 |
|---|---|---|---|
| `backfill` | `backfill` | 用户指定 `[from, to]`，且 `from ≥ now - history_depth_ms` | 无；按页推进，失败可从 `batch_items.to_ts` 续 |
| `backfill` | `incremental` | `[last_closed_ts + interval, now]` | 取该 dataset 已落库的 `max(ts) where closed` |
| `forward` | `capture` | 只允许 `from = now`，`to` 为空 | 每次取「最近 N 条」，按 `ts > last_ts` 去重后追加 |
| `snapshot` | `sample` | 定时单次 | 无游标，主键 `ts` 去重 |
| 任意 | `manual` | 单次，不写游标 | 无 |

两条硬规则：

1. **`forward` 类型不允许出现 `from`**。请求里带了 `from` 就直接 400，并在错误里写明原因：
   「该源该类型只提供最近窗口，历史无法回补」。否则用户今天看不到报错，三个月后
   才发现自己的逐笔数据只有从建任务那天开始——那是数据工坊最不该犯的错。
2. 增量必须**从最后一根已收盘 bar 之后**开始，且允许重叠 1 根（幂等由主键保证）。
   不能从 `max(ts)` 之后开始——最后一根可能未收盘，重拉会覆盖它，这是对的；
   跳过它则会留下一个永远不更新的 bar。

### 5.5 写入通路

原 Rust 实现逐行 `stmt.execute`，实测 11 ms/行（≈90 行/秒），25 万行 45 分钟。
Python 侧改成 **临时 CSV + `COPY` + 区间删除**：

```
payload → 临时 CSV（NULL 写空字段）
        → COPY _ohlcv_stage FROM 'tmp.csv' (FORMAT CSV, HEADER false, NULLSTR '')
        → DELETE FROM ohlcv WHERE (instrument_id, interval, price_type, ts) BETWEEN ...   ← 按分组区间
        → INSERT INTO ohlcv SELECT * FROM _ohlcv_stage
        （整体一个事务）
```

**实测：512,981 行 / 5.8 秒（≈88k 行/秒）**，正好覆盖「一年 1m 单标的」这个量级。

有两条坑必须写进代码注释，否则下一个人会重新踩：

1. **不能用 `executemany` 写 B 表。** 当前 DuckDB（1.5.6，与 Rust 端 engine 一致）
   在参数化插入累计约 1 万行时**直接 abort 进程**——不是抛异常，是解释器消失、没有 traceback。
   `ON CONFLICT DO UPDATE` 也一样。`COPY` 绕开参数绑定，所以稳定。
2. **幂等靠先删区间，不靠主键冲突处理。** 批量装载时不能用 `ON CONFLICT`，那就必须
   在插入前把该 `(instrument, interval, price_type)` 的 `[min_ts, max_ts]` 删掉。
   只删区间而不是整表：一个标的的多周期、多通道数据在同一个表里。

`vwap` 在写入时算一次（`turnover / volume`），两条为空就写 NULL——不参与任何聚合。

去重：主键 `(namespace, instrument_id, interval, price_type, ts)` 保证读侧不会出现重复，
写侧由上面的区间删除保证。批内重复（同一页里出现两个同 ts）由 `duplicates()` 计数并记进
批次的 `rows_rejected`。

### 5.6 质量门禁（按 data_type）

统一流程（**交易所同步与 CSV 必须走同一条**，这是当前最大的架构不一致）：

```
fetch → imported 表 → 门禁检查 → 通过：提升到 bronze（同一事务）
                               → 不通过：留 imported，批次标记 rejected，等人工确认
```

| 检查项 | 适用类型 | 判定 | 严重度 |
|---|---|---|---|
| `gap` | ohlcv, funding, oi | 应有网格上缺失的点；返回**区间明细**而非只给数量 | warn |
| `sample_gap` | orderbook, ticker（`snapshot`） | 相邻采样间隔 > `2 × poll_interval_ms`；**不按 native 粒度判** | warn |
| `stale` | ohlcv, funding, oi | 最新点距 now 超过 2×interval（24/7 市场） | error |
| `stale_sample` | orderbook, ticker | 最新采样距 now > `2 × poll_interval_ms` | error |
| `ohlc_violation` | ohlcv | `low > min(open,close)`、`high < max(open,close)`、`high < low` | error |
| `null_value` | 全部 | 必填列为 NULL | error |
| `duplicate` | 全部 | 主键重复（写入前的行内重复） | warn |
| `non_monotonic` | 全部 | 时间戳非严格递增 | error |
| `zero_volume` | ohlcv | `volume = 0` 占比 > 1% | warn |
| `price_jump` | ohlcv | 相邻 bar 涨跌幅 > 阈值（默认 20%，可配） | warn |
| `cross_interval` | ohlcv | N 根细周期聚合 ≠ 粗周期（high/low/volume） | warn |
| `unclosed_bar` | ohlcv | 落库了 `closed=false` 的数据 | error |
| `interval_irregular` | funding | 相邻结算间隔偏离标称值 | warn |
| `rate_outlier` | funding | `\|rate\| > 1%` | warn |
| `window_boundary` | `forward` 类型 | 数据起点距 now 的跨度 < 预期（说明中间丢过窗口） | warn |

三条元规则：

- **只报告，不自动修**。自动插值会制造出「看起来完整」的假数据，比缺口更危险。
- **`null_policy` 显式**：字段缺失 ≠ 0。检查器读 NULL 直接跳过该项，不参与均值与比率。
- **网格来源必须按 `mode` 选**：`backfill`/`incremental` 用 `native_interval`，
  `snapshot` 用 `poll_interval_ms`，`forward` 用「历史最短连续段」。这一条不加，
  盘口/逐笔一接入就会满屏假缺口——这是 REST-only 最容易被忽略的连带成本。

`cross_interval` 是最该补的一项：上次造数时手工验过 5m 与 5 根 1m 的 high/low/volume
2,015/2,015 全等，但这件事从来没变成常规检查——而它正是「回测与实盘对不上」的常见来源。

---

## 6. API 设计（datahub 现有风格，不加版本前缀）

| 方法 | 路径 | 用途 | 对应页面 |
|---|---|---|---|
| `GET` | `/api/instruments?venue=&type=&search=` | 标的检索（来自 `instruments` 表） | 接入向导 |
| `GET` | `/api/sources` | `{"sources":["binance",…]}`（旧契约）+ `items`/`all`（完整记录） | 接入·数据源卡片 |
| `POST` | `/api/sources` | 新增/编辑数据源（provider、venue、base_urls、config） | 数据源管理 |
| `DELETE` | `/api/sources/:id` | 删除（`enabled=0` 保留的替代方案） | 数据源管理 |
| `POST` | `/api/sources/:id/probe` | 连通性 + 能力探测，结果写回 `capabilities` / `status` / `notes` | 卡片「自检」 |
| `GET` | `/api/jobs` / `POST /api/jobs` | 任务列表 / 新建（不支持组合直接 400，附原因） | 接入·任务表 |
| `POST` | `/api/jobs/:id/run` | 立即执行，返回 `rows_written` / `batch_id` / `error` | 任务行操作 |
| `GET` | `/api/batches?limit=&status=` | 批次审计 | 接入·批次表 |
| `GET` | `/api/datasets?market=&namespace=` | 资产台账（含 `instrument_id` / `inst_type` / `health` / `legacy_key`） | 资产·台账 |
| `GET` | `/api/datasets/summary?dataset_key=` | 行数/区间/新鲜度/覆盖率 | 资产·详情面板 |
| `POST` | `/api/quality` · `GET /api/quality/latest?dataset_key=` | 体检结果（findings 数组，落 `quality_findings`） | 资产·体检清单 |
| `GET` | `/api/coverage?dataset_key=` · `/summary` · `POST /refresh` | 日×时矩阵 / 汇总 / 重算 | 资产·热力图 |
| `POST` | `/api/query` | SQL（只读守卫，字符串字面量先剥离再判断） | 数据页 |
| `POST` | `/api/import/csv` · `/import/promote` | 文件导入 / 提升到 bronze | 接入·文件源 |

**路径参数里的 `dataset_key` 一律用 query 而不是 path**：键里含 `:` 和 `/`，
放进 path 段会被路由和编码各坑一次。

破坏性变更说明：`/api/datasets` 返回的 `dataset_key` 是 6 段格式（旧的是 4 段）。
响应里**同时**给 `dataset_key`（新）与 `legacy_key`（旧），前端在一个版本内兼容。

---

## 7. 前端映射（按已确认的原型）

| 原型元素 | 数据来源 |
|---|---|
| 数据源卡片的状态灯 / 延迟 | `GET /api/sources` + `POST /api/sources/:id/probe` |
| 卡片上的「主域不可达已切镜像」 | `sources[].notes[]`，由适配器在探测时填写 |
| 任务表进度 / 下次触发 / 上次结果 | `GET /api/jobs`，`last_batch` 内联最近一批 |
| 批次审计的「门禁结论」 | `import_batches.gate_passed` + `reject_detail` |
| 资产 KPI | `GET /api/datasets/summary`（一次算好，不在前端循环） |
| 台账每行的「体检」徽章 | `datasets.health`（由最近一次 `quality_findings` 汇总） |
| 详情面板 10 项体检 | `GET /api/quality/:key`，基础 6 + 进阶 4 |
| 缺口明细 + 「补数」 | `quality_findings[code=gap].detail`，补数 = 跳到接入建 `backfill` 任务 |
| 覆盖率热力图 | `GET /api/coverage/:key?month=` |
| 新建任务抽屉的预估 | 前端按 `rows = span / interval` 算；耗时常数**由后端给**（`GET /api/sources` 带 `rows_per_sec`），不写死 11 ms |
| 抽屉里的「数据类型」选项 | 由 `sources[].per_type` 生成：不支持的组合**直接不出现**（不是禁用后报错） |
| 抽屉里的「起始日期」下限 | `min = today - history_depth_ms`；选了更早的日期，就地提示「该源最早可到 X，更早区间会留空」 |
| `forward` 类型的起止控件 | 「起始日期」置灰并写死为建任务日，副文案说明「该源只提供最近窗口」 |
| `snapshot` 类型的「调度」 | 只给轮询周期（≥ `min_poll_interval_ms`），不给「一次性回补」选项 |

最后一条要注意：原型里把「11 ms/行」写在前端只是演示。真实实现应由后端上报当前写入吞吐，
否则批量写入改造完成后，界面还在吓唬用户「要跑 7 分钟」。

还有一个反向约束：**前端的可选性必须来自后端能力矩阵，不能在前端写死「支持哪些类型」**。
写死的话，等某家改了历史深度、或者我们给 Bybit 补上 `fromId` 游标，就要改两处；
更糟的是两边漂移之后，界面允许的组合后端不支持，用户点「创建任务」得到一个 400——
这正是上一轮 `unauthorized` / 5 个 tab 那种「界面与后端不同源」的同类问题。

### 7.1 这一版实际落地的映射（与上表的差异要如实记）

上表是按原型设计的目标态，其中几个端点这一版还没有。页面只用**已存在**的接口，
所以下面这些是"用现有接口做了等价的事"，不是"忘了做"：

| 原目标 | 实际实现 | 说明 |
|---|---|---|
| `GET /api/datasets/summary` 一次算好 KPI | 前端用 `GET /api/datasets` + `domain.summarizeDataset` 汇总 | 数据集量级是几十条；等上了分页再挪到后端 |
| `GET /api/quality/:key` | `GET /api/quality/latest?dataset_key=` + `POST /api/quality` | 前者取最近一次结论，后者触发重跑 |
| `import_batches.gate_passed` + `reject_detail` | `GET /api/imports`（同一批审计记录） | 字段名是 `imports`，返回体已有 gate 结论与坏行样本 |
| 任务表的「下次触发 / 最近一批」 | `GET /api/jobs` 的 `cron` / `last_run_at` / `last_error` | `last_batch` 还没内联，故不显示批次号 |
| 新建抽屉的耗时预估（`rows_per_sec`） | 不做预估 | 后端还没上报吞吐；宁可不显示，也不写死 11 ms 骗人 |
| 抽屉里的「数据类型」选择 | 固定 `ohlcv` | V1 只做 K 线；等 `data_type` 扩到资金费率再放开 |
| 「补缺口」跳转建 backfill 任务 | 未做 | 需要 quality finding 带区间明细，属 V1.1 |
| 「导出报告」 | 前端导出 JSON（台账 + 覆盖率 + 体检摘要） | 不是 PDF；要 PDF 走办公技能那条链 |
| 「全部自检」 | 逐个 `POST /api/sources/:id/probe`（仅启用项） | 串行，避免同时打多家交易所 |
| 「全库巡检」 | 逐个 `POST /api/quality`（上限 50 个数据集） | 单个体检失败不中断整轮 |

另：**K 线已不属于数据工坊**。`data` 包里的自绘指标图（MA/EMA/BOLL + 量 + MACD）整体迁到
`market`，与 Vela 看盘图并列成「看盘 / 指标」两个视图，共用同一份 `/api/bars` 数据。
数据工坊只剩「数据接入」「数据资产」两个页签。

---

## 8. 迁移与分期

**V1（已完成，可运行）**

1. `services/datahub-api`：FastAPI + ccxt + DuckDB/SQLite，HTTP 契约与旧 Rust 服务一致。
2. `sources` 表 + 5 条种子（binance / okx / bybit / official / replay），CRUD + 探测端点。
3. `ohlcv` / `instruments` 建表；旧 `bars` 启动时一次性迁移（`ohlcv` 非空即跳过）。
4. `ohlcv` 写入走 COPY（512k 行 / 5.8s），`vwap` 写入时派生。
5. 任务：`backfill` / `incremental`，逐 symbol 展开，写批次审计 + 目录 + 覆盖率 + 六项体检。
6. carrier 改为启动自带 Python（`runtime/python/python.exe -m app`），端口文件契约不变。
7. CSV 导入（`imported`）与手动提升（`promote` → `bronze`）。

**V1.1（前端按原型重做）**

8. 抽 `quantdsh-datahub-client` 共享包（端点发现 + 401 重试 + 类型），`data` 与 `market` 各删一份重复实现。
9. ~~单插件 `quantdsh-plugin-dataworkshop`，两个页签（数据接入 / 数据资产）按已确认原型实现；
   K 线整块从数据工坊移到 `market`。~~ **已做**：没有新建插件包（`data` 包本身就是这个身份），
   直接把它改成双页签；K 线迁到 `market`（见 §7.1 末段）。
10. ~~数据源管理页：新增/编辑/启停/探测，编辑 `base_urls`、`inst_types`、`url_overrides`。~~ **已做**：
   新增/编辑/启停/探测都有了；`url_overrides` 仍走 `config.url_overrides` 的 JSON，没做独立表单。
11. 抽共享包这一步**还没做** —— 现在 `data` 与 `market` 各自仍有一份 `datahub/client.ts`。

**V2（数据类型扩展）**

12. `funding_rates` / `open_interest`（Binance 的 OI 历史只有 30 天，越早接越好）。
13. `trades` / `orderbook`（逐笔按区间拉、盘口只做采样，默认关闭）。
14. 轮询调度器（整分 +3s、抖动、全局并发上限、落后转小段回补）。
15. 跨周期一致性检查、缺口 → 回补闭环、体检历史对比。

**明确不进路线图**：WebSocket 接入。数据层的最后一环是「可重放的历史底表」，
策略模块若需要实时信号，是另一条链路，它消费 WS 但不往 `ohlcv` 里写。

---

## 9. 运行与打包（自带 Python）

| 项 | 做法 |
|---|---|
| 解释器 | `scripts/prepare-python-runtime.mjs` 下载 python-build-standalone 的 `install_only` 版本，解包到 `build/python-runtime/python` |
| 依赖 | 用该解释器 `pip install -r services/datahub-api/requirements.txt`，装进它自己的 `site-packages` |
| 裁剪 | 装完删掉全部 `*.pdb`、`include/`、`libs/`（Python 与 MSVC 的调试符号和编译头，运行期一次都不会读） |
| 打包 | `tauri.conf.json` 的 `bundle.resources` 把 `build/python-runtime` 映射到 `runtime/`，`services/datahub-api/app` 映射到 `datahub-api/app` |
| 启动 | carrier spawn `runtime/python/python.exe -s -B -m app --data-dir <dir>`，工作目录 `datahub-api` |
| 隔离 | `-s` 忽略用户 site-packages，`-B` 不写 `.pyc`；用户系统里的 Python 完全不参与 |
| 开发 | 自动回退到 `services/datahub-api/.venv`（仅 debug 构建），或用 `QUANTDSH_PYTHON` / `QUANTDSH_DATAHUB_DIR` 覆盖 |
| 排障 | 服务的 stderr 落到 `<data_dir>/datahub.log`；release 构建没有控制台，这是唯一线索 |

**DuckDB 版本必须锁在 `1.5.6`**：用户机上已有的 `market.duckdb` 是 Rust 端
（`duckdb = "=1.10506.1"`，即 engine 1.5.6）写出来的，存储格式版本不同就打不开。
Rust 那个 crate 现在已经删除，但这个 pin 不能动——它锁的是**磁盘上的文件格式**，不是依赖习惯。
这一条写在 `requirements.txt` 的注释里。

**体积（实测）**：解包 + 装依赖后 **291 MB**，裁掉调试符号与头文件后 **206 MB**。构成大致是
`ccxt` 67 MB、`cryptography` 10 MB、`pip` 10 MB、`duckdb` 1 MB、`Tcl/Tk` 11 MB，其余是标准库。
这是「用户不用装 Python」的代价。`ccxt` 是最大单项——它把 100+ 家交易所的元数据都随包发，
换掉它的唯一办法是退回逐家手写适配器，与 V1「先跑通」的目标冲突，因此接受。

**启动耗时（实测）**：206 MB 的运行时冷启动到 `/health` 可用 **2.7 s**（导入 fastapi + ccxt + duckdb）。
carrier 的超时按 45 s 给，因为用户机上 Defender 首次扫描刚解包的文件会明显变慢。

**两条验收命令**（都不依赖本机 Python）：

```powershell
# 1. 打包树上真实起一次服务：等 datahub.json → /health → 401 → 种子源 → schema
python scripts/verify_bundled_runtime.py

# 2. 安装包里的解释器能 import 全部 wheel（verify-packaged.mjs 会跑同一条检查）
runtime\python\python.exe -s -B -c "import fastapi, uvicorn, ccxt, duckdb"
```

### 9.1 为什么安装包里有两个 Python（不共用 `primary-runtime`）

安装包里确实已经有一份 CPython：`quantdsh-runtime/primary-runtime/dependencies/python`
（3.12.14，146 MB，预装 numpy 2.3.5 / pandas 3.0.1 / lxml / Pillow / python-docx / python-pptx /
openpyxl / XlsxWriter），那是桌面端给 Office 类技能用的**官方运行时**。既然有，为什么还要在
`runtime/python` 再放一份 206 MB？四条理由，任一条都足以否掉共用：

1. **它是官方产物，带来源回执**。`primary-runtime/runtime.json` + `.quantdsh-source-artifact.json`
   记录了上游 exe 的 url / size / sha512 与 `payloadDigest`，`verify-packaged.mjs` 会核这份回执。
   往里面 `pip install` 之后，树里已经不只是官方 payload，回执却仍声明自己是——这是本仓库
   所有校验机制专门要防的那种「自欺」。
2. **它的包集合是被声明的**。`runtime.json` 明确写了 `pythonPackages`（numpy 2.3.5、pandas 3.0.1…），
   而 Office 计算链路会**硬校验**：`official-operation.mjs` 里 `runtime.pythonDistributions.numpy !== '2.3.5'`
   直接抛 `HOST_DEPENDENCY_UNAVAILABLE`。`pip install fastapi ccxt` 会连带升级 `typing_extensions`
   这类公共依赖，一旦动了 numpy，坏的是 Office 导出，报错却指向「专业计算不可用」——排查成本极高。
3. **修复流程会整棵替换**。`runtime_pack.rs` 的离线修复/还原是以 `primary-runtime` 为单位的，
   我们把 wheel 塞进去，任何一次修复都把它们抹掉，行情服务随即起不来，而且是在「修好了别的东西」之后静默失败。
4. **生命周期不同**。官方运行时的版本跟 `upstream.json` 走（Node / pnpm / Python 三者一起升），
   行情服务的依赖跟 `requirements.txt` 走。绑在一起意味着上游一次 Python 小版本升级就可能把
   `duckdb==1.5.6` / `ccxt` 的 cp312 wheel 撞坏，而这条链路上没有任何人会预期到这种耦合。

**如果将来必须省这 100 MB**，可行的折中是「共用解释器、不共用环境」：用
`primary-runtime/dependencies/python/python.exe` 启动我们的服务，我们自己的一份 `site-packages`
放在 carrier 拥有的目录（`<data_dir>` 下的 pyenv，或 resources 里我们自己的目录），靠 `PYTHONPATH`
注入。官方环境一个字节都不动，修复流程也不碰它。代价是多一套「首启解包 / ABI 校验（cp312 vs
上游换 3.13）」逻辑。V1 不做：现在 installer 主体已经 ~1 GB，省 100 MB 换这四个风险不划算。

**两个 Python 各自服务的对象互不重叠**，所以「重复」只在磁盘上，不在运行时：
`primary-runtime/dependencies/python` 跑 Office 脚本，`runtime/python` 跑行情服务，两者不互相 import。

---

## 10. 风险与未决

| 项 | 影响 | 处理 |
|---|---|---|
| `api.binance.com` 本机不可达 | 现货同步跑不通 | 已确认 `data-api.binance.vision` 同构可用（仅 `/api/v3/*`）；适配器做候选 base |
| **本机对 OKX / Bybit 也不可达**（`www.okx.com` 代理返回 502，`api.bybit.com` 直连超时） | P0 若只在家用网络验收，OKX/Bybit 会一直是「源不可达」 | 联调与验收需在能直连的网络下做；`/api/sources` 的探测结果要能区分「本机网络问题」与「交易所故障」 |
| Binance OI 历史仅 30 天 | 错过即永久缺失 | 进 P0，日更任务 |
| 各交易所限速会变 | 大批量任务被拒 | 限速参数配置化，从响应头读用量；不写死 |
| DuckDB 单文件并发 | 写入时外部工具打不开 | 已有只读守卫；P2 考虑按年分库 |
| 逐笔体量 | 磁盘爆炸 | 默认不接 + 保留期策略 |
| CCXT 版本演进 | 字段增删 | 只对齐核心字段，`raw` 存原文，不怕它变 |
| **只用 REST → 没有实时性** | 最新一根 K 线/指标最多滞后一个轮询周期（默认 1m 任务 ≈ 60~90s） | 接受这个 SLA，并在界面标注「最新值可能滞后」；实时需求不进数据层 |
| **只用 REST → 盘口不可重建** | 无法做「某分钟当时的完整盘口」回放，只能看采样点 | 类型上标 `snapshot`，详情面板明确写「采样快照，非连续盘口」 |
| **只用 REST → 部分类型只能向前采集** | Bybit 逐笔一旦漏停就永久断档 | `forward` 模式 + `window_boundary` 检查 + 界面写死起始日；停机超阈值时告警而不是静默 |
| **能力矩阵与实现漂移** | 界面允许、后端不支持，用户建出空跑任务 | 能力矩阵只由后端 `Capabilities` 生成；`POST /api/jobs` 服务端二次校验 |
| 强平数据缺失 | 极端行情研究少一个维度 | 已明确不做（REST 无公开源）；需要时由用户在策略侧自行接 WS，不入本库 |

---

## 附：为什么最终用了 CCXT + Python sidecar

这一节记录一次**明确的决策反转**，因为读者会看到前后两版结论不同。

v1 的结论是「P0 只有三家交易所，原生适配器（Rust）比引 sidecar 便宜」，理由成立但前提是
「数据层长期只有三家」。v2 的前提变了：**数据源本身要入库、要能加交易所**。在「加一家交易所 =
改一次代码 + 发一次版」与「加一家交易所 = 加一行配置」之间，前者对产品是硬伤，所以换成后者的收益
不在代码量，而在**发版节奏**。

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| A. Rust 原生适配器 | 无额外进程；限速/分页可控；与 daemon 共享连接池与日志 | 每家要写一次映射（约 200 行），新增交易所必须发版 | 放弃（v1 曾采用） |
| B. Node sidecar 跑 `ccxt` | 一次接入 100+ 家 | 与平台运行时（Node）争用；js 版 ccxt 的市场元数据同样几十 MB | 放弃 |
| C. Python FastAPI + `ccxt` | 生态最全；**新增交易所零代码**；写入用 DuckDB 官方绑定 | 多一套运行时（+206 MB）；打包链路变长 | **采用** |

三条纪律把 C 的风险压住：

1. **只用同步 API + `anyio.to_thread.run_sync`**。ccxt 的 async 客户端把 session 绑在事件循环上，
   而 FastAPI 进程会常驻、会重载，绑错层就是难查的「偶发超时」。
2. **禁止 `watch*`**：数据层只接 REST，这一点与 v1 一致，没有因为换库而放松。
3. **`options.fetchMarkets` 必须按 inst_type 收窄**：ccxt 默认一次拉全部市场类型，任一类不可达
   整个任务就失败——`binance/spot` 曾因为去拉合约接口而整体报错。

**如果将来要收回体积**：把 `ccxt` 换成只依赖 HTTP 的薄客户端，具体做法是保留 `Provider`
接口与 `sources` 表不变，只替换 `app/providers/ccxt_source.py`；接口隔离就是为了留这条路。

