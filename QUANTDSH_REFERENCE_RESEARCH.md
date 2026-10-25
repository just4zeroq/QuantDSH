# QuantDSH 参考项目调研：QuantDinger / QuantMind

> **调研目的**：用 `QuantDinger` 与 `QuantMind` 两个开源量化项目完善 QuantDSH 的功能设计。
> **本轮范围**：只调研与产品设计，**未改动任何代码**，也未修改既有两份规划文档。
> **调研日期**：2026-10-25

---

## 0. 结论速览（TL;DR）

| 项目 | 定位 | 许可证 | 对我们的用法 |
|---|---|---|---|
| **QuantDinger** | 本地优先、隐私优先、自托管的**量化交易基础设施**（数据在撮合执行的一侧） | **Apache 2.0**（宽松，商业友好） | ✅ **可直接借鉴代码与设计**：数据接入、多交易所/券商适配、回测成本模型、多智能体 + 记忆反思 |
| **QuantMind** | AI 原生**量化研究与交易平台**（Qlib + RD-Agent + 对话机器人） | **AGPL v3**（强 Copyleft，含网络服务条款） | ⚠️ **只借鉴概念，禁止复制代码**：因子自主进化闭环、质量检测闭环、Parquet+DuckDB 存算、交互形态 |
| QuantStudio（前参考） | DSH 运行时之上的量化工作台 | GPL-3.0 / PandaAI 商业双授权 | ⬇️ 建议**降级为次要参考** |

**最重要的一条**：QuantDinger 是 **Apache 2.0**，比 QuantStudio 的 **GPL-3.0 / PandaAI 商业双授权**干净得多。
我们此前「决策 2/5：直接复用 QuantStudio 技能集」是在没有这个选项时做的。**建议修订为：以 QuantDinger 作为主参考，QuantStudio 降为次要参考**（详见 §6）。
这直接化解了之前那条最大的合规隐患——但按你的既有决策，我不会擅自改，第 6 节列了修订建议待你确认。

---

## 1. QuantDinger 概览

### 1.1 是什么
**本地优先（local-first）、隐私优先、自托管的量化交易基础设施**。跑在用户自己的机器/服务器上，策略、交易日志、API 密钥、分析结果全部留在本机。

- 技术栈：Python（**Flask**）+ PostgreSQL（多用户）+ Redis（可选）/ Vue 2 + Ant Design Vue + KlineCharts。
- 部署：Docker Compose。
- 代码许可 **Apache 2.0**，但**名称/Logo/品牌受商标约束**（再发布修改版需移除/替换品牌标识，商业展示需授权）。

### 1.2 能力矩阵

| 能力 | 说明 |
|---|---|
| 可视化 Python 策略工作台 | 用标准 Python 写指标/策略（Pandas/Numpy/TA-Lib），在内置 K 线上可视化调试信号；AI 辅助编程 |
| 完整交易生命周期 | 指标 → 策略配置（仓位/止损止盈）→ 回测 + **AI 建议参数调整** → 执行 |
| 实盘执行 | 加密：Binance/OKX/Bitget/Bybit/Coinbase/Kraken/KuCoin/Gate/Bitfinex；美股港股：IBKR；外汇：MT5 |
| 信号通知 | A股/期货等无法实盘的市场，走 Telegram/Discord/Email/SMS/Webhook |
| AI 多智能体投研 | 基于 TradingAgents 架构：分析师 → 多空辩论 → 交易员 → 风险辩论 → 最终决策 |
| **AI 记忆增强** | 每角色本地 SQLite 记忆库 + **反思闭环**，检索增强；支持自动验证与手动复盘 |
| 通用数据引擎 | CCXT（100+ 交易所行情）、Yahoo/Finnhub/Tiingo（美股）、AkShare（A/港股）、OANDA（外汇期货）；内置代理支持 |
| 策略运行时 | 线程池执行器、**重启后自动恢复运行中策略**、订单队列后台线程 |

### 1.3 值得注意的实现细节（已核实）

- **数据源抽象**（`app/data_sources/factory.py`）：按市场类型 `get_source(market) → BaseDataSource`，统一 `get_kline(symbol, timeframe, limit, before_time)` / `get_ticker(symbol)`；异常时**优雅降级**（返回 `[]` 或 `{'last': 0}`）而不是抛穿调用栈。这个「契约统一 + 失败软着陆」的模式非常适合我们的数据中枢。
- **交易适配工厂**（`app/services/live_trading/factory.py`）：按 `exchange_id` 分派;各交易所 `base_url` 有内置默认值；**现货/合约分支**（`mt == "spot"` 走 spot client，否则合约）；IBKR/MT5 用**惰性导入**（缺少 `ib_insync`/`MetaTrader5` 时才报错），避免可选依赖拖累主包。这正是我们要的「多券商适配层」的现成骨架。
- **回测**（`app/services/backtest.py`）：已核实建模了 `commission`（默认 0.001）、`slippage`、`leverage`，支持多周期（mtf）模拟，产出权益曲线、成交列表、问诊指标。**缺口**：没有资金费率/永续合约基差模型（加密场景必需，属于我们的补位点）。
- **多智能体**（`app/services/agents/README.md`）：分析师（市场/基本面/新闻/情绪/风险）→ 看涨/看跌研究员辩论 → 交易员决策 → 风险分析师三方（激进/中性/保守）辩论 → `final_decision`。返回结构化 JSON（`recommendation`/`confidence`/`trading_plan`/`reasoning`），便于下游执行与留痕。
- **团队/权限模型缺失**（已核实：`app/` 下无 `Team`/`TeamMember`/`team_id`）：它有 PostgreSQL 多用户账号，**但没有「团队」与「功能授权」概念**。

---

## 2. QuantMind 概览

### 2.1 是什么
面向个人量化研究者/投研团队/机构的**一体化 AI 原生量化交易平台**，深度集成微软 **Qlib** 与 **RD-Agent**（其 `alphaagent/` 即 RD-Agent 的一个演进分支），以 **QuantBot** 作为自然语言总入口。

- 技术栈：Python FastAPI 微服务（api / engine / trade / stream 四服务）+ PostgreSQL + Redis；Electron + React + TypeScript 前端；Docker Compose。
- 许可：**AGPL v3**。
- 支持市场：A股、港股、美股、期货、**区块链**。

### 2.2 能力矩阵

| 能力 | 说明 |
|---|---|
| **QuantBot 对话总入口** | 自然语言跑完「取数→分析→选股→回测→报告→模拟下单」；SSE 流式；多会话隔离 |
| **IM 远程指令** | 同一机器人暴露到微信/企业微信/QQ/飞书/钉钉等 17 类渠道，手机发一句话即可远程执行，结果回推聊天窗口 |
| **量化技能池** | 技能以 `SKILL.md` 打包，一键导入启用；研报 MD/PDF 自动归档到「技能中心→报告档案」 |
| 市场与数据 | QuantDB 数据中枢，**300+ 维预计算特征**（含 L1/L2 微观结构、资金流）；**Parquet + DuckDB** 列式存算；7×24 RSS 舆情监控 + 事件实体匹配 + 情绪量化 |
| **因子自主进化** | 集成 RD-Agent（AutoAlpha 2.0）：自然语言假设 → 因子公式合成 → 遗传演化回测 → 优选入库 |
| **13 种模型工场** | LightGBM/XGBoost/CatBoost/GRU/LSTM/ALSTM/Transformer/TabNet/TCN/NativeTFT…；Stacking 集成（时序 OOF + Ridge 元学习器）；本地 Docker 或纳管自建 GPU 节点（AutoDL） |
| **批量推理与信号** | 全市场截面打分、Top-N 推荐、多信号融合；**生产质量闭环**：每日真实 Rank IC 自动回填 + SHAP 特征重要性 + PSI 数据漂移双通道告警 |
| Qlib 回测 | 事件驱动，TopkDropout 多因子选股；细粒度费率/滑点/涨跌停模拟；多维收益归因与风险指标 |
| 模拟交易与风控 | 本地 T+1 撮合，还原涨跌停/停牌/整手/佣金印花税过户费/滑点；**启动前 Preflight Check**；持仓与订单全生命周期 + 每日资金快照 |

### 2.3 值得注意的实现细节（已核实）

- **研发闭环**（`alphaagent/components/workflow/rd_loop.py`）：`RDLoop` 的五个阶段——
  `propose`（假设生成）→ `exp_gen`（假设转实验）→ `coding`（写码）→ `running`（执行）→ `feedback`（反馈）→ 压入 `trace.hist`。
  这是一个**可迭代、会累积 Trace 的研发循环**，而不是一次性出报告。对「投研智能体」来说这是骨架级的设计。
- **Parquet + DuckDB 的组合**在仓库里到处出现（同步脚本、特征快照、氛围结算等），并配套 `bronze/silver` 分层重建脚本——**验证了我们「DuckDB 本地数据湖」的决策是对的**。
- **市场适配器**（`backend/services/engine/rd_agent/market_adapters/`）含 `a_share.py` / `hong_kong.py` / `us_stock.py` / `futures.py` / **crypto.py**——连 RD-Agent 那条链路都已经考虑加密市场。
- **质量闭环三件套**（Rank IC 回填 + SHAP + PSI）是它最有工程价值的部分，也是多数量化项目缺失的「模型上线后是否还有效」的答案。

---

## 3. 许可证与可用性判定（务必先看清）

| 项目 | 许可证 | 能否直接复用代码 | 关键约束 |
|---|---|---|---|
| **QuantDinger** | **Apache 2.0** | ✅ **可以** | 需保留 LICENSE/NOTICE 与版权署名；**不得使用「QuantDinger」名称/Logo/品牌**暗示背书或来源；再发布修改版一般需移除/替换其品牌标识（除非获书面商业授权）。专利条款友好。 |
| **QuantMind** | **AGPL v3** | ❌ **不可以** | 第 13 条「网络交互」条款：**只要让用户通过网络与修改版交互，就必须向这些用户提供源代码**。我们的客户端 + 未来服务端都会触发。任何「参考其实现后写出相似代码」的做法都有传染争议。 |
| QuantStudio（前参考） | GPL-3.0 / PandaAI 商业双授权 | ⚠️ 需逐项核实 | GPL 传染 + PandaAI 商业授权问题，闭源商用需单独评估。 |

**AGPL 的防护建议（对我们的硬约束）**：
1. **不复制、不翻译 QuantMind 的源码**；只采用**文档/README 层面**的产品概念与行业通用做法（如「Rank IC 回填 + PSI 漂移告警」属通用 ML 监控实践，实现方式我们自己写）。
2. 若将来有人参与过 QuantMind 源码阅读，量化 family 的实现应尽量由未接触者按概念实现，避免「实质性相似」争议。
3. 保留调研记录（本文档）以证明来源是公开 README 级信息而非代码。

**结论**：代码级的可借鉴资产几乎全部来自 **QuantDinger**；QuantMind 提供的是**产品功能清单与工程范式**。

---

## 4. 功能映射到我们的四大支柱

> 标记：`[D]` = QuantDinger 可直接借鉴 · `[M]` = QuantMind 概念借鉴 · `[O]` = 我们自己补（两者都没有）

### 4.1 数据中枢 Data Hub

| 借鉴点 | 来源 | 落地建议 |
|---|---|---|
| 按市场分派的数据源工厂 + 统一 `get_kline/get_ticker` + 失败软着陆 | `[D]` | 直接采用该抽象；Rust 侧用 trait `DataSource`，按 market key 注册实现 |
| CCXT 100+ 交易所行情 / AkShare / Yahoo / Finnhub / OANDA 的多源清单 | `[D]` | 作为 connector 清单参考；** crypto 优先**按我们的决策先做交易所公开 API → DuckDB |
| Parquet + DuckDB 列式存算 + bronze/silver 分层 | `[M]` | 采纳分层：原始落 bronze（不可变），清洗/对齐/复权后入 silver（Feature Store） |
| 300+ 维预计算特征 + 特征注册表（含 L1/L2、资金流） | `[M]` | 建 `feature_registry`（元数据 + 版本 + 血缘），规模随 crypto 场景重定 |
| 7×24 RSS 舆情 + 事件实体匹配 + 情绪量化 | `[M]` | crypto 场景换成：交易所公告/项目动态/社媒情绪；进入 **News/Sentiment 分析师**输入 |
| **链上数据 / 永续资金费率 / 期现基差** | `[O]` | **两者都没做好的加密专属维度** → 我们的差异化补位 |

### 4.2 策略工坊 Strategy Workshop

| 借鉴点 | 来源 | 落地建议 |
|---|---|---|
| **研发闭环 5 阶段**（propose→exp→code→run→feedback→Trace） | `[M]` | 作为「投研流水线」骨架：每次实验进 Trace，支持迭代而不是一次性生成 |
| 因子自主进化（自然语言假设 → 公式合成 → 遗传演化 → 入库） | `[M]` | 因子实验室的核心机制；先做「候选因子 + 评估」，再上演化循环 |
| 多模型工场 + Stacking 集成 + GPU 调度 | `[M]` | 首版收敛到少量模型（如 LightGBM + 1~2 个），保留 `ModelAdapter` 接口 |
| **质量闭环：Rank IC 回填 + SHAP 重要性 + PSI 漂移告警** | `[M]` | **强烈建议采纳**（改为 crypto 截面 IC 口径）。这是「模型上线后还灵不灵」的答案，多数竞品没有 |
| 回测成本模型：commission + slippage + leverage + 多周期 | `[D]` | 采纳并**补齐加密专属**：资金费率、永续基差、逐仓/全仓保证金、穿仓保险基金 |
| 收益归因与风险指标全景 | `[M]` | 按账户/合约/品种/策略多维度拆解（我们原方案已有） |
| 涨跌停/停牌/整手等规则化撮合 | `[M]` | 加密场景不需要，但**跨资产扩展（A股/期货）时预留规则插件位** |

### 4.3 交易台 Trading Desk

| 借鉴点 | 来源 | 落地建议 |
|---|---|---|
| 交易所适配工厂：按 id 分派 + base_url 默认值 + 现货/合约分支 | `[D]` | 直接照搬形态到 Rust `BrokerAdapter` trait + factory |
| 9 家加密交易所 + IBKR + MT5 的覆盖清单 | `[D]` | 印证「多资产多券商」路线；首版 crypto，后续按此清单扩 IBKR/MT5 |
| 惰性导入可选依赖 | `[D]` | Rust 侧用 feature gate 表达，避免可选连接器拖累二进制体积 |
| 策略运行时：线程池 + **重启后自动恢复** + 订单队列 | `[D]` | 采纳；对我们的桌面端尤其重要（用户会关整机） |
| T+1 模拟撮合 + 费用还原 + **启动前 Preflight Check** | `[M]` | Preflight 思路很好：实盘/模拟启动前自检（余额、权限、网络、时钟、参数完整性），不过不启动 |
| 仿真/实盘统一接口，实盘默认 opt-in | `[O]` | 沿用我们既有决策（仿真+实盘同步，实盘默认关闭） |
| 信号通知渠道（Telegram/Discord/Email/SMS/Webhook） | `[D]` | 可并入企业版「告警推送」，见 §4.5 |

### 4.4 投研智能体 Quant Agents

| 借鉴点 | 来源 | 落地建议 |
|---|---|---|
| 多智能体流水线（分析师 → 多空辩论 → 交易员 → 风险辩论 → 终决） | `[D]` | 采纳角色编排，并**改造为加密版**：分析师加「链上/资金费率/衍生品结构」等维度 |
| 结构化输出（`recommendation`/`confidence`/`trading_plan`/`reasoning`） | `[D]` | 采纳；便于下游「AI 结论作为交易过滤器」与审计留痕 |
| AI 结论作为**市场过滤器**（情绪不一致则不允许交易） | `[D]` | 很有价值的风控形态，可做成策略开关 |
| **记忆增强 + 反思闭环**（见 §5.4 详述） | `[D]` | **重点采纳**：让智能体随时间变好，而不是每次从零推理 |
| 对话式总入口 + 技能池 + 报告归档 | `[M]` | 采纳「技能池 + 产物归档」；对话入口本来就在 DSH 运行时里 |
| IM 远程指令（微信/QQ/飞书/钉钉…） | `[M]` | 短期**不建议**（合规与账户安全风险：免登录端口 + 凭据外置），可作为企业版远期候选，见 §4.5 |

### 4.5 企业版（用户 / 团队 / 功能挂钩）

**核实结论：两个项目都没有「团队 + 功能授权」模型。** QuantDinger 只有 PostgreSQL 多用户账号；QuantMind 有角色但无团队级功能授权。
→ **我们的企业版需求（增加用户 / 创建团队 / 增加团队人员 / 功能挂团队）确实是真差异化，不是重复造轮子。** 既有的 client-server 设计保持不变。

可借鉴的两点：
- `[D]` 多用户账号与数据隔离的模型（PostgreSQL 用户维度隔离）→ 我们的服务端单 org 多 user 已覆盖。
- `[M]` **IM / 通知推送**思路 → 企业场景可转成「团队告警推送」：策略异常、风控触发、回测完成、等级 IC 漂移告警，推送到团队负责人。**但应先做应用内/邮件/Webhook，不要一上来接微信机器人**（QuantMind 自己也标注了「对外端口 0.0.0.0 且免登录」的安全提示）。

功能授权 key 建议（服务端 `feature_key` 枚举初稿，对应我们的支柱）：
```
data-hub / factor-lab / research-agents / backtest
paper-trading / live-trading / risk-alerts
team-collab / content-publish
```

---

## 5. 重点设计：可直接搬的形状

### 5.1 数据源抽象（照 QuantDinger）

```
trait DataSource {
    fn market(&self) -> Market;                       // Crypto | USStock | AShare | Futures | Forex
    fn get_kline(&self, symbol, timeframe, limit, before) -> Result<Vec<Bar>>;
    fn get_ticker(&self, symbol) -> Result<Ticker>;
}

fn get_source(market: Market) -> Arc<dyn DataSource>;  // 注册表 + 缓存实例
```
要点：**统一返回契约** + **失败软着陆**（取不到返回空/默认值并记日志），与 QuantDinger 一致——这能让上层不会因为单一数据源故障整体崩溃。

### 5.2 券商适配层（照 QuantDinger factory）

```
trait BrokerAdapter { fn place_order/.../fn fetch_positions/fn fetch_balance }

fn create_client(cfg: ExchangeConfig, market_type: MarketType) -> Box<dyn BrokerAdapter>
    // binance | okx | bybit | bitget | gate | kucoin | kraken | coinbase | bitfinex
    // spot ⇄ perp 分支；传统券商 IBKR / MT5 走 feature gate 惰性启用
```
要点：**按 id 分派 + 内置默认 endpoint + spot/perp 分支 + 可选依赖惰性启用**。

### 5.3 回测成本模型（在 QuantDinger 基础上加密补齐）

| 成本项 | QuantDinger | 我们需要 |
|---|---|---|
| 手续费 commission | ✅ | ✅ |
| 滑点 slippage | ✅ | ✅（可升级为按盘口深度建模） |
| 杠杆 leverage | ✅ | ✅ |
| **永续资金费率 funding** | ❌ | ✅ **补** |
| **期现基差 / 结算价** | ❌ | ✅ **补** |
| 保证金与强平 | ❌ | ✅ **补**（逐仓/全仓） |

### 5.4 记忆增强 + 反思闭环（QuantDinger 最有价值的单点）

**机制**：每个 agent 分析前先从本地记忆库检索相似历史经验注入 prompt，分析后把结论入库；事后验证收益并写回，形成闭环。

- **检索打分**（建议照搬并保留可调权重）：
  ```
  score = w_sim·similarity + w_recency·decay(half_life) + w_returns·tanh(returns)
          − timeframe_mismatch_penalty
  ```
  默认 `0.75 / 0.20 / 0.05`，半衰期 30 天。`tanh` 压缩收益避免极值支配；`timeframe` 不一致扣分。
- **相似度**：默认本地哈希向量 cosine（无外部向量库依赖），退化为 `difflib` 文本相似 —— **零外部依赖**，契合我们「本地优先」。
- **两条写入通道**：① 自动反思 worker（事后拉最新价验证结论对错，写回）；② 手动复盘 API（用户录入真实成交结果）。后者更可靠，建议两个都做。
- **存储**：与我们已经决定的**服务端 SQLite + 客户端 SQLite 缓存**天然契合（QuantDinger 就是用 SQLite：`data/memory/*.db`）。
  → 建议：记忆库落在**服务端**（跨会话、跨设备共享），客户端缓存只读副本用于离线推理。这需要我们在服务端契约里**新增两三个 API**（`GET/POST /api/agent/memory`, `POST /api/agent/reflect`）。

**为什么重要**：这让「投研智能体」从「每次从头推理」变成「会积累经验的投委会」，是可防御的产品差异，而不是又一个 chat 面板。

### 5.5 质量检测闭环（照 QuantMind 概念，自己实现）

上线后的模型/因子每天自动：
1. **Rank IC / ICIR 回填**（crypto 用截面 IC 口径）→ 有效性跟踪；
2. **SHAP / 特征重要性** → 解释性与归因；
3. **PSI 数据漂移** → 分布变化时告警。

三者构成「模型还在赚钱吗、为什么、是不是数据变了」的答案。属通用 ML 实践，我们自己实现。

---

## 6. 对我们既有决策的影响与修订建议

| 既有决策 | 现状 | 调研后建议 | 是否自动改 |
|---|---|---|---|
| **2/5 量化技能集：直接复用 QuantStudio** | 以 QuantStudio 为主参考 | ⬅️ **建议改：主参考换成 QuantDinger（Apache 2.0）**，QuantStudio 降为次要参考。理由：QuantDinger 覆盖数据/执行/多智能体三块且许可干净得多，直接消除 GPL/PandaAI 合规风险 | ❌ **待你确认** |
| 1/5 数据：DuckDB 本地数据湖 | 已定 | ✅ **被 QuantMind 验证**（Parquet+DuckDB 是成熟组合）；追加建议：补 bronze/silver 分层 | 概念性补强 |
| 3/5 首版资产：加密货币 | 已定 | ✅ 进一步确认：两者都没做好 **资金费率 / 期现基差 / 链上数据** → 明确作为我们的差异化补位 | — |
| 4/5 交易：仿真 + 实盘同步 | 已定 | ✅ 追加：采纳 QuantMind 的 **Preflight Check**（启动前自检，不过不启动）作为安全护栏 | 建议新增 |
| 5/5 profile 命名 quantdsh | 已定 | 不受影响 | — |
| 企业版 10 项决策 | 已定 | ✅ 不受影响；且**两家都无团队/授权模型**，验证我们的企业版是真差异化。追加：`/api/agent/memory`、`/api/agent/reflect` 两个备选 API | 备选新增 |

**新增建议（可直接并入规划）**：
1. 引入 **Preflight Check** 作为实盘/模拟启动门槛。
2. 引入 **记忆增强 + 反思闭环**（服务端记忆库 + 客户端只读缓存 + 两个新 API）。
3. 引入 **质量闭环**（IC 回填 + SHAP + PSI 告警）。
4. 回测补齐 **资金费率 / 基差 / 保证金**，这是 crypto 相对两者的显著补强。

---

## 7. 建议的功能增量清单

> `[D]` QuantDinger 可借鉴 · `[M]` QuantMind 概念 · `[O]` 我们补齐

**Data Hub**
- `[D]` 按市场的数据源抽象与失败软着陆
- `[M]` Parquet+DuckDB + bronze/silver 分层
- `[M]` 特征注册表（元数据/版本/血缘）
- `[M]` 舆情事件脉 + 情绪量化（crypto 改为项目/社媒口径）
- `[O]` 链上数据 / 资金费率 / 期现基差

**Strategy Workshop**
- `[M]` 研发闭环 propose→exp→code→run→feedback→Trace
- `[M]` 因子自主进化循环
- `[M]` 多模型适配接口（先少后多）
- `[M]` **质量闭环：IC 回填 + SHAP + PSI 漂移告警**
- `[D]` 回测成本模型（费/滑/杠杆）
- `[O]` 资金费率 / 基差 / 保证金 / 强平

**Trading Desk**
- `[D]` 券商适配工厂（9 家 + IBKR + MT5 形态）
- `[D]` 策略运行时：线程池 + 重启自动恢复 + 订单队列
- `[M]` **Preflight Check** 启动前自检
- `[D]` 信号通知渠道（团队告警）
- `[O]` 高亮：仿真 + 实盘，实盘默认 opt-in

**Quant Agents**
- `[D]` 多智能体编排（分析师→辩论→交易员→风险辩论→终决）
- `[D]` 结构化决策输出 + AI 作为交易过滤器
- `[D]` **记忆增强 + 反思闭环**（重点）
- `[M]` 技能池 + 产物归档
- `[M]`（远期）IM 团队推送 —— 需先解决安全与合规

**Enterprise**
- `[O]` 用户/团队/功能授权（真差异化，两家皆无）
- `[D]` 多用户账号与数据隔离
- `[M]` 团队告警推送

---

## 8. 风险与合规清单

1. **AGPL 传染（最高优先）**：绝不复制 QuantMind 源码，禁止「读实现后再写」；只采用 README/文档级概念；保留本调研记录作为来源证明。
2. **QuantDinger 商标**：可用代码（Apache 2.0），但**不得展示 QuantDinger 名称/Logo**，再发布修改版需移除/替换其品牌标识；保留 LICENSE/NOTICE 与版权署名。
3. **QuantStudio 许可**：若最终仍部分复用，需逐项核实 GPL-3.0 / PandaAI 商业双授权与发行路径。
4. **crypto 实盘合规**：沿用既有决策（实盘默认关闭、opt-in、按地区/构建可关闭）；分发前评估。
5. **IM 机器人安全**：QuantMind 自身提示其对外端口默认 `0.0.0.0` 且免登录。若将来做企业 IM 推送，须先做鉴权、IP 限制与凭据隔离。
