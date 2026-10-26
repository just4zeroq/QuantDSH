# QuantDSH 产品方案（终版）

> **版本**：FINAL **v1.3** · 2026-10-26（v1.2 基础上吸收第四轮决策：免费额度口径、试用、席位失效语义、**代码公开前提下的防破解授权设计**）
> **地位**：本文档是 QuantDSH **整体产品的单一事实来源**，收编并取代此前三份规划的决策汇总地位。
>
> ### v1.3 变更（第四轮拍板）
> | # | 议题 | 结论 | 影响面 |
> |---|---|---|---|
> | A | **免费版数据额度口径** | **按币种切分**（symbol 白名单），不按时间范围切 | §3.7 新增；§3.3 / §6.1 改写；原"滚动近 1 年"方案作废 |
> | B | **免费 / 订阅的功能切法** | **本期不定**，先把功能做完整，**做完后统一规划切分** | §3.3 层级列改标「待规划」；功能实现不依赖该切法 |
> | C | **试用（trial）** | **不做，不保留任何 trial 字段与机制** | §4.3 移除 `source='trial'`；不设试用期逻辑 |
> | D | **席位到期** | **立即失效**（对到期时刻精准到秒） | §3.5 / §3.6 / §5.1 重写；不再有"7 天宽限"语义 |
> | E | **防破解（代码公开）** | 授权判定必须按"客户端完全在攻击者手里"设计，**价值放服务端 + 有效期置于服务端签发物 + 判定下沉 native 层** | §3.8 新增整节（本轮最大改动）；§11 增风险项 |
>
> ### v1.2 变更（第三轮拍板 + 参考项目代码级调研）
> | # | 议题 | 结论 | 影响面 |
> |---|---|---|---|
> | A | **组织成员关系** | **一个用户只属于单组织**（`users.org_id` 非空单值），不做多组织切换，多组织永久移除出待定项 | §4.3 增加单组织约束；§3 授权求解简化；§12.2 第 5 项关闭 |
> | B | **调研口径** | **调研不再设开源许可门禁**：三家参照的功能、代码结构、商业模式一律完整调研；许可证仅作事实记录，不作为借鉴前置条件 | §8 由"许可证屏障"改为"借鉴策略 + 事实记录"；调研附录升级为 v2 |
> | C | **商业模式纳入规划** | 在企业/权益模型之外，补齐**订阅套餐、支付闭环、key 生命周期、增长回路**的参考结论 | §1.4 新增商业化机制；§4 新增套餐与支付契约 |
>
> ### v1.1 决策（第二轮六项拍板，继续有效）
> | # | 议题 | 结论 | 影响面 |
> |---|---|---|---|
> | 1 | 开源许可证 | **暂不处理**（后续如需对外分发再评估） | 许可证不再参与任何技术取舍；仅在 §8.3 留事实记录 |
> | 2 | 计量 / 计费 | **暂不做**，只做"订阅权益判定"，不引入计量与账单 | §4 移除计费模块 |
> | 3 | 团队共享区 | **服务端托管** | §2.3 / §4 锁定 |
> | 4 | 版本与商业模式 | **在个人版基础上发展**：部分功能免费、部分需订阅 key；**订阅分为个人订阅与团队订阅** | §1.4 / §3.5 / §3.6 / §5.1 重写 |
> | 5 | 首版交易所 | **Binance + OKX** 两家 | §3.3 / §6.3 / §10 收敛 |
> | 6 | 技术基座 | **沿用既有 DSH 运行时 profile（`workdsh`）扩展**，不另起 `quantdsh` profile | §2.1 / §5.2 / §5.4 修订（原 v1.0 的 profile 切换改造取消） |
>
> 除上述六点外，v1.0 已锁定的决策（量化 9 项 + 企业 10 项）**继续有效**。
> - `QUANTDSH_QUANT_PLAN.md` —— 降为**功能模块附录**（详细三级菜单与来源标记）
> - `QUANTDSH_ENTERPRISE_PLAN.md` —— 降为**企业契约附录**（SQLite DDL / token / 路由细节）
> - `QUANTDSH_REFERENCE_RESEARCH.md` —— 降为**调研附录（v2）**：三家参照的完整功能清单 + **代码级实现要点** + **商业模式调研**。许可证仅作事实记录，不作借鉴门禁
> - 三者内容仍然有效；与本文冲突时**以本文为准**。
>
> **本轮范围**：产品方案设计，不涉及代码实现。

---

## 1. 产品是什么

### 1.1 一句话定位

**QuantDSH = 本地优先的量化投研桌面客户端 + 官网经营的组织/团队/授权平台。**

用户在本机完成「取数 → 研究 → 因子/策略 → 回测 → 交易 → 复盘」的全部闭环，数据、凭据、策略与产物留在自己机器里；组织身份、成员、团队与功能授权由官网统一管理，通过一枚 key 下发到客户端离线执行。

### 1.2 我们要解决的三个真问题

| 现状痛点 | 参照项目中被验证的事实 | QuantDSH 的答案 |
|---|---|---|
| 数据被绑定 / 不归自己 | QuantStudio 绑 PandaData；QuantMind 绑 QuantDB | **数据自有**：可插拔连接器 + DuckDB 本地数据湖 |
| 投研一次性、不累积 | QuantStudio 无智能体记忆；QuantMind 无企业级归档 | **会积累经验的投研智能体**：记忆 + 反思闭环 + Trace |
| 团队用不了 / 不敢共用 | QuantStudio 明文"共享部署无成员隔离"；另两家无团队授权模型 | **用户 → 团队 → 功能**单一授权轴 + 本地优先离线门禁 |

### 1.3 用户与场景

| 角色 | 诉求 | 关键产品承诺 |
|---|---|---|
| 个人量化研究者 | 一人跑通全流程、数据留本机、不被锁死 | 桌面单实例、离线可用、数据自有 |
| 团队负责人（Lead） | 带几个人一起做、控制谁能用实盘 | 团队级授权、实盘默认关闭且显式 opt-in |
| 组织管理员（Owner/Admin） | 集中建账号、分组、按需开功能、可审计 | 官网管理 UI + 结构化 key + 审计流水 |
| 投研/IT 合规方 | 数据不外泄、行为有留痕 | 本地优先 + 完整运行留痕与审计底账 |

### 1.4 商业模式：免费起步，订阅解锁（单一客户端包）

> 不做"个人版 / 企业版"两个发行物。**一个安装包**，装好就能免费做研究与仿真；想上生产和团队，才需要一枚订阅 key。

```
免费（不登录、可离线）
   └─ 研究对象级能力：取数 → 因子 → 回测 → 仿真 → 归因
      ★ **具体哪些功能免费、如何分档，本期不定，做完功能后统一规划**（v1.3 决策 B）
      ★ 已定的只有**额度口径**：按**币种**切（§3.7），不按历史长度或限速

订阅 · 个人 key     权益主体 = user，换机重装不丢
订阅 · 团队席位     权益主体 = team，成员占席后继承，释放/到期即失效
   └─ 生产级能力与协作：加密专属数据 / crypto 成本补完 / 实盘 / 多智能体与记忆
      / 质量闭环 / 团队共享区 / 市场发布 / 告警推送
```

- 有效权益 = **免费 ∪ 个人订阅 ∪ 各所属团队**，同一功能取最高 `access_level`（详见 §3.5）。
- **每条权益都带 `expires_at`，到期立即失效**（v1.3 决策 D，见 §3.5.1）。
- 由此带来的产品承诺：**免费功能即使断网、无 key、无快照也必须可用**——这是 §3.6 离线策略的第一条。

#### 1.4.1 商业化机制（三家调研后的定案）

| 机制 | 方案 | 参考依据与取舍理由 |
|---|---|---|
| 计量 / 按次积分 | **不做**。服务端只判定"有没有权益"，不统计用量 | QuantDinger 的积分制（AI 分析 10 / 回测 3 / 指标创建 0）与其自托管定位绑定；我们是单一客户端 + 订阅制，引入积分会让"断网可用"的承诺失效 |
| 订阅套餐（Plan） | **做**。套餐 = 一组 `feature_key` + 有效期 + `auto_renew` | QuantMind `SubscriptionPlan{code, price, currency, interval, features[]}`，与我们的权益模型同构且已被工程验证 |
| 支付闭环 | **做**。官网下单 → 拉起支付 → 异步回调**幂等** → 按回调中的 `plan_key` 激活订阅 | 对齐 QuantMind 支付宝流程：`/orders → payUrl → /alipay-notify → 校验 trade_status → 幂等 → 激活`，并重定向前前端成功页 |
| key 生命周期 | 明文**仅返回一次**、支持轮换、支持吊销；吊销经 **增量吊销列表**传播，下次任何联网动作即失效，并用 `must_refresh_before` 收紧窗口（不再有固定宽限） | QuantMind `POST /api-keys`（明文一次性）+ `ApiKeyRotateSecretResponse`（轮换）；我们把"宽限"换成"签发时写死的短期有效期"（§3.8 L3） |
| 席位 | 团队订阅 → 成员占席 → 占席后才继承团队权益；**释放或到期即失效**（到期精度到秒，离线也生效） | 自有强约束；**不发行"团队公共 key"**，否则无法把下单行为归因到个人，审计形同虚设 |
| 增长回路 | 注册礼包 + 邀请同组织成员；赠送的是**权益有效期**而非代币 | 借鉴 QuantDinger `CREDITS_REGISTER_BONUS` / `CREDITS_REFERRAL_BONUS` 的获客思路，但不引入积分概念 |
| **授权防破解（代码公开）** | **见 §3.8**：服务端签名快照 + **有效期写在签发物里**（使"到期立即失效"离线也成立）+ 判定下沉 native + 高价值数据内容加密 + 实盘在线票据；**明确不做侵入式 DRM** | 客户端在攻击者手里，"能不能绕过"是伪命题；正确目标是**把价值放服务端**并提高篡改成本 |
| 免费白名单 | 免费功能子集走客户端内置 `FREE_KEYS`，**不进授权快照**（具体分档待商业规划，实现上不得依赖某个具体名单） | 借鉴 QuantDinger 把 `cost_indicator_create` 设为 0 的手法，但我们要做到断网也免费 |

#### 1.4.2 商业模式横向对照（知彼知己）

| | QuantDinger | QuantMind | QuantStudio | **QuantDSH** |
|---|---|---|---|---|
| 交付 | 自托管 Web（Docker） | SaaS 平台（+OSS 镜像） | 运行时 Web 工作区 | **桌面客户端 + 官网** |
| 收费单位 | 按次积分 + VIP | **订阅套餐（月）** | 商业授权 | **订阅（个人 / 团队席位）** |
| 支付 | Telegram 人工充值 | 支付宝 | 商务洽谈 | 官网支付（对齐支付宝流程） |
| 权益表达 | 功能 → 积分值 | `plan.features: list[str]` | 授权范围 | **`feature_key` + `access_level` + 隐含前置** |
| 组织 | 无团队 | `tenant_id` | 无成员隔离 | **单组织 + 团队 + 席位** |
| 免费额度 | 部分功能免积分 | 套餐外不可用 | 无 | **按币种切（§3.7）** |
| 离线 | 不适用 | 不支持 | 部分 | **✅ 本地优先 + 签名快照（有效期写在签发物里）** |
| 防滥用 | 无 | 仅在线校验可用量 | 无 | **签名快照 + 吊销增量 + 设备绑定 + 数据加密**（§3.8） |

> **我们的相对优势**：QuantMind 的 `check_feature_access` 只有"有/无"两级、feature 之间无前置关系、权益不挂团队、每次都查库、无离线能力。我们的模型在同一象限上是它的加强版——这正是差异化所在。

---

## 2. 产品全景：三层 + 两端 + 三个存储区

### 2.1 总体架构

```
                         ┌────────────────────────────────────────────┐
                         │            官网 / 平台（服务端）              │
                         │            Rust + Axum + SQLite             │
                         │                                            │
                         │  账号中心  组织  团队  成员  功能授权  Key 发放 │
                         │  审计流水  团队共享区  策略/因子市场  版本分发  │
                         └────────────────────┬───────────────────────┘
                             ① key 登录        │        ② 授权快照 / 共享资产
                                (org.user.xxxx)│           / 记忆同步 / 更新
                         ┌────────────────────▼───────────────────────┐
                         │        QuantDSH 桌面客户端（Tauri carrier）   │
                         │                                            │
                         │  key 会话 · can_access 门禁 · 离线降级       │
                         │  entitlement 缓存(SQLite) · keychain        │
                         └────────────────────┬───────────────────────┘
                              ③ 托管 profile   │  workdsh:// 协议
                         ┌────────────────────▼───────────────────────┐
                         │  运行时 profile `workdsh`（沿用）+ quant 工作区 │
                         │                                            │
                         │  数据中枢 · 策略工坊 · 交易台 · 投研智能体      │
                         │  本地数据湖 DuckDB · 本地记忆 SQLite           │
                         └────────────────────────────────────────────┘
```

**为什么要三层而不是两层**：运行时 profile 是所有业务 UI 的容器，carrier 只负责身份、门禁、窗口与运行时生命周期。把授权与 UI 分开放，才能保证「换工作区不换授权体系」「离线也能门禁」「实盘能力可按地区/构建开关整体关掉」。

> **决策（v1.1）：沿用 `workdsh` profile 扩展，不新建 profile。**
> 量化能力作为该 profile 内的 **quant 工作区**叠加。收益：运行时契约天然不必动、既有用户平滑升级、省掉一整套 profile 打包与迁移；
> 代价：量化数据与既有工作数据共处同一 profile 根，需要**目录命名空间隔离**（见 §5.4），且 carrier 侧原本计划的 `PROFILE_NAME` 切换改造**取消**。

### 2.2 职责边界

| 能力 | 官网/平台 | 客户端 carrier | workdsh profile（quant 工作区） |
|---|---|---|---|
| 账号、组织、成员 | ✅ 唯一权威 | ❌ | ❌ |
| 团队、功能授权、Key | ✅ 唯一权威 | 只读快照 | ❌ |
| 授权判定执行 | 定义授权 | **本地执行 `can_access`** | 调用门禁 |
| 数据接入与存储 | ❌ | 目录与生命周期 | ✅ 连接器 + DuckDB |
| 研究 / 回测 / 交易 UI | ❌ | 窗口壳 | ✅ |
| 实盘凭据（交易所 API） | ❌ 不落服务端 | OS 凭据库 | 运行时使用 |
| 记忆与反思 | 服务端跨会话共享库 | 缓存只读副本 | 写入与检索 |
| 更新与回滚 | 版本清单 | 校验 + 切换 + 回滚 | profile 备份 |

> **红线**：实盘凭据、交易所密钥、策略源码**绝不上传服务端**。服务端只持有身份与授权。

### 2.3 三个存储区（重要）

| 区 | 位置 | 引擎 | 内容 | 归属 |
|---|---|---|---|---|
| **本地私有区** | `%APPDATA%\WorkDSH\profiles\workdsh\quant\` | DuckDB + Parquet | 行情、资金费率/基差、因子、回测产物、交易所凭据（OS keychain）、本地记忆 | 用户独占，永不同步秘密 |
| **企业授权区** | 同一 profile 下的 `entitlement.db` | SQLite | `{user, org, teams, grants}` 快照 + 拉取时间 | 服务端派生，本地缓存 |
| **团队共享区** | 服务端 SQLite/对象存储 | Axum | 共享策略/因子/报告/发布物 + provenance | 组织/团队维度，按需拉取只读副本 |

> 数据根目录沿用已确认决策：默认 `%APPDATA%\WorkDSH\profiles\quantdsh`，**位置可配置**；不改动 `%APPDATA%\WorkDSH` 根。

### 2.4 运行时契约红线（不得改动）

`workdsh://` scheme、`workdsh.localhost` host、`workdsh-runtime` 资源目录、`workdsh-*` npm 包名、
`runtime_pack::PRODUCT = "WorkDSH"`、`%APPDATA%\WorkDSH` 数据根。
改这些会破坏 dsh 兼容与离线包 manifest 校验。品牌变更只发生在**用户可见文案与 `identifier`**。

---

## 3. 统一身份与授权模型

### 3.1 设计原则：单一授权轴

> **角色决定「能不能改授权」，团队决定「能不能用功能」。**

市面上常见做法是双授权轴（角色一份、团队一份），结果是权限不可推理、离职即扩散。QuantDSH 只允许一条链：

```
User ──TeamMembership(Lead|Member)──▶ Team ──FeatureGrant(view|use|manage)──▶ Feature
```

用户的有效权限 = 其所属所有团队的功能授权**取最高 access_level**。

### 3.2 角色矩阵

| 角色 | 作用域 | 能做什么 | 不能做什么 |
|---|---|---|---|
| **Owner** | 组织 | 全部管理动作、解散组织、审计查看 | — |
| **Admin** | 组织 | 建用户/团队、调成员、分配功能授权、吊销 key | 解散组织；改 Owner |
| **Lead**（团队内） | 自己带的团队 | 增减本团队成员、读写共享区、发本团队告警 | 改功能授权；跨团队操作 |
| **Member** | 无管理权 | 按团队授权使用功能 | 任何管理动作 |

> Owner/Admin 对各团队隐含 Lead 权限，因此他们**也通过团队获得功能**，不为管理员单开后门——权限永远可推理。

### 3.3 `feature_key` 终版术语表

> 这是**服务端授权粒度**的正式定义，也是「功能和团队挂钩」的完整清单。

| feature_key | 名称 | 覆盖能力 | 隐含前置 | 备注 |
|---|---|---|---|---|
| `core.app` | 客户端基础使用 | 启动、设置、查看本地数据 | — | 吊销=封禁 |
| `data.hub` | 数据中枢 | Binance + OKX 现货/永续 K 线 ticker、同步任务、数据湖读写、本库预览 | — | **可用标的集受额度约束**（§3.7） |
| `research.factor` | 因子实验室 | 因子库、IC/IR、衰减、分组收益、换手评估、HTML-MD 报告 | `data.hub` | 研究闭环核心 |
| `research.backtest` | 回测引擎 | 事件驱动；commission / slippage / leverage | `data.hub` | 两家交易所足够跑通 |
| `analysis.performance` | 绩效归因 | 净盈亏曲线、品种归因、毛/净标注、缺费待核算、采样缺口检查、CSV 导出 | `research.backtest` | 严格口径是信任项 |
| `research.agents` | 投研智能体 | 对话入口 + Skill / Expert / Expert Team 三级模型 | `core.app` | 一等公民 |
| `trade.paper` | 仿真交易 | testnet / 纸面撮合 + Preflight Check | `research.backtest` | 无真实资金 |
| `data.crypto-native` | 加密专属数据 | 资金费率 / 期现基差 / 标记价 / 清算 / 深度快照 / 链上指标 | `data.hub` | 三家参照项目都没做好的差异化 |
| `research.backtest-pro` | 回测加密补完 + 多周期 | mtf 多周期；资金费率、期现基差、逐仓/全仓保证金、强平与结算 | `research.backtest` | crypto 场景刚需 |
| `research.factor-pro` | 因子生命周期与血缘 | 健康矩阵、拥挤度告警、退役/重建候选、provenance、handoff_card、一键复现 | `research.factor` | 生产化必备 |
| `research.agents-pro` | 多智能体流水线与记忆 | 五段流水线、结构化决策、**记忆增强与反思闭环**、服务端记忆共享、加密分析师扩展 | `research.agents` | 让智能体"会积累经验" |
| `research.automl` | 自主进化与模型工场 | 因子进化循环、`ModelAdapter`、研发闭环 Trace | `research.factor` | 排在后期 |
| `quality.monitor` | 生产质量闭环 | 每日 Rank IC/ICIR 回填 + SHAP 重要性 + PSI 漂移告警 | `research.factor` | 回答"上线后还灵不灵" |
| `trade.live` | 实盘交易 | 真实资金下单（**首版 Binance + OKX**） | `trade.paper` | 敏感；Preflight + opt-in，**在线票据**（§3.8 L5） |
| `trade.auto-exec` | 自动下单 | 免逐笔确认自动执行 | `trade.live` | 最敏感，需二次确认 + 在线票据 |
| `team.collab` | 团队共享区 | 共享策略/因子/报告的读写（写需 manage） | `core.app` | 服务端托管（已锁定） |
| `market.publish` | 发布到市场 | 组织内/对外发布带 provenance 的研究 | `team.collab` | 后期开放 |
| `notify.alerts` | 告警推送 | 应用内 / 邮件 / Webhook | `core.app` | IM 通道远期且需鉴权 |

> **共 18 项功能键。**
> **功能本身的免费 / 订阅划分（以及套餐组合）本期不做，先把功能做完整，做完后统一规划。**（v1.3 决策 B）
> 因此本节不再标注层级；`can_access` 的实现必须做到**切法上线时只改服务端套餐配置 + 一处白名单常量，不动功能代码**（见 §3.4 的判定入口单一化）。

**规则**
1. 授予某权益时，其隐含前置**自动授予**（管理端展示但不需单独勾选）。
2. `trade.live` 与 `trade.auto-exec` 即使被授予，也**必须通过 Preflight Check** 且用户在本地显式 opt-in 才可用；还可按地区/构建整体关闭。
3. `access_level` 语义：`view` 只读与查看报告；`use` 可运行与下单；`manage` 可改他人可见配置（如共享区写入、市场发布）。
4. **免费功能键不进授权快照**：由客户端内置白名单判定，因此**无网、无 key、无快照时依然可用**——这是"装完就能用"的基础。

### 3.4 客户端门禁 `can_access`（判定入口单一化）

```
can_access(feature_key) -> Decision {
  level, expires_at, source }

  1. if feature_key ∈ FREE_KEYS:                 # 切法待定，本期 FREE_KEYS 可为空集
       return { level: 内置级别, expires_at: None }        # 离线永可用
  2. 载入 entitlement 快照 → 验签失败则 fail-closed 到步骤 1
  3. 逐条权益过滤：now() >= entry.expires_at → 立即丢弃（§3.5.1 到期即失效）
  4. 展开本人 team 集合 + 个人订阅 → 取所有命中项 → 应用隐含前置展开
  5. 返回最高 access_level；无命中 → UI 置灰并提示如何获得
```

**工程约束**
- **唯一判定入口**：全应用不得出现第二处 `feature_key` 判断；UI 置灰、数据同步、连接器下单、回测开关全部走这个函数。
- **判定放在 native carrier（Rust）而非 web profile 的 JS**：profile 是 web 资产，改一行 JS 就能解锁；下沉到编译产物显著提高篡改成本（§3.8 L1）。web 侧只负责展示 `can_access` 的返回值。
- 切法上线时只改 `FREE_KEYS` 常量 + 服务端套餐配置，**功能代码不动**。

### 3.5 订阅模型与失效语义

> 本期**不做计量与账单**（已决策），服务端只回答：**这个用户此刻有没有这项权益，到什么时候为止**。

| 概念 | 定义 | 落地形态 |
|---|---|---|
| **免费权益** | 与账号无关的能力基线（切法待定） | 客户端内置白名单，离线永可用 |
| **个人订阅** | 权益主体 = **user** | 用户一枚个人 key；换机器/重装后权益不变 |
| **团队订阅** | 权益主体 = **team**，带**席位 seat** | 成员占席后继承团队权益；席位可释放回收 |
| **两者可叠加** | 同一用户可同时持有个人订阅并加入多个团队 | 有效权益 = free ∪ 个人 ∪ 各团队，**同 key 取最高级别** |

#### 3.5.1 有效期：每条权益自带 `expires_at`（支撑"到期立即失效"）

服务端结算快照时，**每条权益项都带自己的过期时间**，取下列最小值：

```
entry.expires_at = min(
    subscription.expires_at,          # 订阅到期
    seat.released_at       or +∞,     # 席位释放
    grant.revoked_at       or +∞,     # 权益被撤销
    snapshot_ttl_cap                  # 快照刷新上限（防永久离线）
)
```

- 客户端**离线也能精准到期失效**：因为 `expires_at` 是服务端**签发时写死在签名快照里**的绝对时间戳，不需要联网比对。
- 配合 §3.8 L3 的单调时钟护栏（防回拨系统时间），"到期立即失效"在离线状态下同样成立。
- **不要在各模块各自比对 `expires_at`**：一律走这一个函数，避免口径分叉。

#### 3.5.2 席位（seat）与"立即失效"

| 事件 | 服务端 | 客户端 |
|---|---|---|
| 席位**到期**（订阅 `expires_at` 到点） | 状态转 `expired` | **到点即刻失效**，不等下次刷新（快照内已含该时刻） |
| 席位**被释放 / 换人** | `seats.released_at` 写值 | 下次联网刷新立即裁剪；离线期间最多存活到快照 TTL 上限 |
| key **被吊销** | 入吊销列表 | 下次联网刷新立即失效；离线期间受 `must_refresh_before` 限制（§3.8 L3） |
| 用户**被禁用** | `users.status='disabled'` | 同上，且禁用即拒绝 `key/verify` |

> 区分三类很有必要：**到期是可预知的**（能写进快照，离线也精准）；**释放与吊销是不可预知的**（必须联网才知道）。
> 所以"立即失效"在到期维度是**绝对精确**的；在吊销维度做到"下次任何联网动作立即生效"，并以较短的 `must_refresh_before` 收紧窗口。
> **trade.live 例外**：开启新的实盘会话永远要求在线票据（§3.8 L5），因此实盘不存在"吊销后还能下单"的窗口。

**为什么不做"整个团队共用一枚 key"**：共享 key 无法追责（分不清是谁下的单），也破坏我们自己的审计要求。每人一枚 key + 席位计数，既可核算也便于吊销。

#### 3.5.3 不做试用（trial）

**不设试用期、不埋 trial 字段、不写 trial 逻辑**（v1.3 决策 C）。
理由：trial 要配套时间锁、到期提醒、转化漏斗与客服，是一整套运营体系；在功能尚未完整前引入只会拖慢主线。
将来若要促销，用**服务端手动 `source='grant'` 发放一段有限期的订阅**即可，不需要专门机制。

### 3.6 离线策略（本地优先）

| 情形 | 行为 |
|---|---|
| **免费权益** | 无网、未登录、无快照时**始终可用**，不受 TTL 影响 |
| 联网 | 启动时 + 定时刷新快照；更新 `fetched_at` 与 `must_refresh_before` |
| **断网（持 key）** | 按签名快照继续门禁，UI 顶部标注「离线授权模式 · 剩余有效期」；**免费功能不受影响** |
| 权益到 `expires_at` | **立即失效**（离线也生效，见 §3.5.1） |
| 超过 `must_refresh_before` 仍未联网 | `trade.live` / `trade.auto-exec` 降级禁用；其余权益继续到各自 `expires_at` |
| key 校验失败 / 被吊销且联网 | 清缓存 → 退回首登；**本地数据不删**（保护用户资产），免费功能继续可用 |
| 时钟回拨 | 检测到 now 早于已记录的 `last_seen_server_time` → 视为篡改风险，敏感项拒绝启动（§3.8 L3） |

### 3.7 免费额度：按**币种**切分（不按时间范围）

> **决策（v1.3 A）**：免费额度差异体现在**可交易的标的集合**，而不是历史长度或下载速率。
> 原方案"滚动近 1 年 + 下载限速"作废——限历史长度会毁掉回测的可信度（样本太短），限速又只是恶心人。
> 按币种切则干净：能研究的标的少几个，但**凡是能取的样本一律完整**，研究结论依然成立。

**额度模型**

| 项 | 定义 |
|---|---|
| `symbol_scope` | 一个**标的集合**，随快照下发并签名：`{ mode: 'whitelist'|'all', symbols: [...], source: 'free'|'plan' }` |
| 口径一致 | 免费额度依附于 `data.hub` 那条权益项，与功能权益**同源、同 `expires_at`、同一份签名** |
| 生效点 | **数据层而非 UI 层**：连接器、同步任务、回测取数、仿真撮合统一先过 `resolve_symbol_scope(symbol)`；UI 置灰只是提示，真正的闸门在读写之前 |

```
resolve_symbol_scope(symbol) -> bool
  scope = entitlement.symbol_scope      # 无快照 → 用内置 FREE_SYMBOLS 常量
  if scope.mode == 'all': return true
  return scope.symbols.contains(symbol)

# 同步任务：DuckDB 写入前逐批校验；回测：加载前校验；仿真：下单前校验
# 越界标的一律在**数据入口**拒绝，并返回可行动的提示（需要哪种订阅）
```

**建议初值（待商业模式确认）**：免费给主流 **BTC / ETH / SOL / BNB / XRP** 等头部若干标的的现货与永续；订阅放开交易所全量标的。
具体名单等商业化规划时定；**实现上必须是服务端可下发配置，不能硬编码到功能代码里**——否则将来调整名单就要发新版客户端。

**为什么这条同时是防破解设计**：它在纯 UI 开关之外提供了一个自然的门禁点——用户只能研究其额度内的标的，
而"额度外的标的要配套什么订阅"这个答案在我们手里。真正防得住的部分，是把高价值数据做成**服务端供给**而非客户端开关（§3.8 L4）。

### 3.8 防破解与授权完整性（**代码公开**前提）

> **威胁模型（必须诚实）**：客户端跑在用户自己的机器上，代码公开、二进制可被改。
> **不存在无法绕过的客户端门禁**——能被绕过的只有时间成本问题。
> 因此本节的目标不是"锁死"，而是三件事：**① 把真正有价值的东西放在服务端；② 让绕过成本高于订阅价格；③ 绝不因防破解伤害付费用户或侵犯隐私。**

#### L0 · 先分类：哪些值得防，哪些不值得

| 类别 | 例子 | 客户端能否绕过 | 策略 |
|---|---|---|---|
| **自证型能力** | 交易所公开 REST 行情、本地因子计算、基础回测 | 能（数据在用户自己手里） | **不做无效投入**，只做 UI 层提示；靠"服务/省时间"而非"锁" |
| **服务端供给型** | 清洗对齐 silver、PIT vintage 包、资金费率/基差派生集、AutoML、记忆同步、团队共享区、市场内容、更新包 | **不能**（服务端不给就是没有） | **主要的商业价值放这里** |
| **高风险操作** | 实盘下单、自动下单、市场发布 | 能开但会被审计发现 | 在线票据 + 事后审计 + 可封禁 |

> 一句话：**把锁做在数据上，不要做在代码上。**

#### L1 · 判定下沉到 native 层

- `can_access` / `resolve_symbol_scope` 实现在 **Rust carrier**（编译产物），web profile 只渲染结果。
- profile（web 资产）里不出现任何 `feature_key` 常量集，避免"改一行 JS 就全解锁"。
- 附带好处：免费可用性在 native 侧保证，不受前端改动影响。

#### L2 · 服务端签名快照（防改快照）

- 快照用 **Ed25519** 签名（JWS，`alg=EdDSA`），客户端内置**公钥**（随版本更新可轮换，`kid` 标识）。
- 快照内容：`{ iss, sub(user_id), org, teams[], grants[{key, level, expires_at}], symbol_scope, device_id, iat, exp, nonce, version }`。
- 验签失败 / 结构非法 / `exp` 过期 → **fail-closed 到免费档**，绝不"忘了就放行"。
- 诚实认知：攻击者可以 patch 掉验签函数本身。所以签名不是终点，它的真正作用是 **L4 的内容加密密钥的唯一来源**。

#### L3 · 时间、吊销与设备的收紧

| 手段 | 做法 | 目的 |
|---|---|---|
| **单调时钟护栏** | 本地持久化 `last_seen_server_time`（SQLite 一行）；若发现 `now()` 早于它 → 判为时钟回拨，敏感项拒绝启动 | 防"把系统时间调回过期前" |
| 短期 `exp` | 快照 TTL 建议 7 天；`trade.*` 类更短（票据见 L5） | 缩小离线窗口 |
| `must_refresh_before` | 服务端下发"最迟须在此时间前联网一次"，超时则敏感权益降级 | 让**吊销**这类不可预知事件接近"立即生效" |
| **吊销增量下发** | `GET /api/revocations?since=<version>` 返回被吊销的 key/权益版本→ 客户端启动时必调（快、可失败降级） | 吊销传播低延迟 |
| **设备绑定** | `key/verify` 携带 `device_id`（本地随机，重装可解绑一次） | 检测 key 共享散播 |
| 并发限制 | 同一 key 的**并发在线设备数**超限 → 软降级（要求重新校验），不硬封 | 防共享但不误伤换机的付费用户 |
| 异常信号 | 短时多地域切换、席位超额、请求速率异常 → 服务端降权 + 人工复核 | 识别批量盗用 |

#### L4 · 内容加密：把门禁绑到数据本身（关键一层）

> 对**服务端供给型数据**，客户端必须从会话里解出内容密钥才能用。改代码绕过 `can_access` 也读不到明文。

```
# key/verify 或专门的密钥交换
sk = HKDF(shared = key_plaintext + server_random, info = dataset_id)   # 每次发放不同
服务端：返回用 key 派生的 datasetKey（或要求在线拉取）
客户端：用它解密本地的 Parquet/SQLite 派生数据集，仅在内存中持有
```

适用范围，**刻意不加密**原始 bronze 行情（本来就是公开数据，加密只损性能）：

| 加密 | 不加密 |
|---|---|
| PIT vintage 包、silver 清洗结果、资金费率/基差派生集、AutoML 产出、专家记忆同步包、市场下载物、更新包 payload | 用户自己的策略源码、交易所公开行情 bronze、回测中间件结果 |

性能取舍：仅对**非海量高价值**派生集加密，避免拖垮 DuckDB 全表扫描。

**这一层能防什么、不能防什么（说清楚）**

| 能防 | 不能防 |
|---|---|
| 改代码绕过 `can_access` 后直接读本地文件内容 | 合法订阅者把明文导出后传播（需配合水印/溯源，见下） |
| 权益到期后继续用旧缓存——密文解不开 | 有人在有效期内完整 dump 出库 |
| 二手传播整份数据目录给别人用 | 逆向者重写整个同步协议（成本已高于订阅价） |

配套：派生数据集按用户/设备**加水印**（例如埋一串不可见的行序扰动或标识列），泄露可溯源；
服务端对同一 key 的数据下载做速率限制，异常批量拉取触发复核。

#### L5 · 敏感操作的在线票据（实盘类）

- 开启**新的**实盘 / 自动下单会话，必须向服务端换取短期 **execution ticket**（建议 10–30 分钟），服务端写审计。
- 已持仓的风控、止损、减仓**本地仍可执行**，票据过期不阻断（避免用户因断网爆仓——这是可用性红线，不能反过来伤害用户）。
- 服务端**绝不接触**交易所密钥与签名（既有红线），因此不下单代签；票据只做"资格证明"。

#### L6 · 交付与更新链路完整性

- 发布包 **Authenticode / notarize 代码签名**；更新强制校验签名与哈希，防止"改过的客户端一直用"。
- 更新通道即防线：被篡改的客户端会在下一次官方更新时被覆盖。
- **明确不做**：反调试、root/越狱检测、进程注入扫描、隐私采集型指纹、第三方 DRM。这类手段伤口碑、易误伤、对我们这种专业小众产品得不偿失。

#### L7 · 商业层面的破解抑制

- **免费版要体面可用**（额度按币种收敛，但功能不残废）→ 降低"被逼破解"的动机。
- 高价值内容由服务端持续生成与更新 → **一次破解不能永久吃**。
- 团队版价值在协作、席位与隔离 → 单人破解拿不到这部分价值。
- 定价与购买流程尽量顺畅（币圈常见的"充值靠找客服"模式坚决不学）。

#### L8 · 违规处置（写清楚，但克制）

服务端侧分级：限制并发 → 要求重新校验 → 软降级到免费档 → 吊销 key（写入 `audit_log`，保留证据）。
**不做**远程删数据、不做"查水表"式上报；任何封禁动作必须可由管理员在官网复核与撤销。

#### 本公司红线（写死，任何人不得突破）

1. 防破解措施**不得**导致付费用户在正常断网时无法使用已付费功能（除 §3.5.2 明确的实盘例外）。
2. 防破解措施**不得**采集与授权无关的用户隐私；指纹只用于识别 key 滥用。
3. 宁可让 1% 的人绕过，**不可**误伤 1 个付费用户或引入侵入式 DRM。

---

## 4. 官网 / 平台（服务端）

### 4.1 模块清单

| 模块 | 优先级 | 说明 |
|---|---|---|
| 账号中心 | P0 | 注册/登录（argon2 或 bcrypt）、找回、邮箱验证；**账号密码只存在于服务端** |
| 组织管理 | P0 | 建组织、Owner 移交、组织级可见配置 |
| 用户管理 | P0 | 管理员增/改/禁用用户；禁用**不清本地数据**，只让会话与 key 失效 |
| 团队管理 | P0 | 建团队、设 Lead、增减成员 |
| 功能授权 | P0 | 按团队勾选 `feature_key` + `access_level`，含隐含前置自动展开 |
| Key 管理 | P0 | 申请/展示一次/吊销/续期；结构化 token；只存 sha256 |
| 审计流水 | P0 | 谁在何时给谁开了什么功能、谁用过实盘、谁发布了共享物 —— 不可删改 |
| 团队共享区 | P1 | 共享策略/因子/报告，带 provenance 与版本（**服务端托管**，本轮锁定） |
| 策略/因子市场 | P2 | 组织内/对外发布、指标展示（Sharpe/IC/回撤/卡玛/胜率）、一键导入 |
| 版本分发 | P1 | 客户端更新清单与强制更新灰度（与客户端"验证后切换"机制配合） |
| **订阅与席位**（不含计费） | P1 | 个人订阅 / 团队订阅 + 席位分配与释放；**权益逐项带 `expires_at`，到期即失效**；只判定权益，不做计量账单 |
| **授权签发与签名**（v1.3） | P0 | Ed25519 签发 entitlement 快照（含 `expires_at` 与 `symbol_scope`）；`kid` 轮换；私钥加密托管，不落明文 |
| **吊销与设备**（v1.3） | P0 | 吊销增量下发、设备绑定与自助解绑一次、并发设备数限制与异常信号 |
| **实盘票据**（v1.3） | P1 | `trade.live` / `trade.auto-exec` 的短期 execution ticket 发放与审计 |
| **服务端数据供给与加密下发**（v1.3） | P1 | silver / PIT / 派生指标经服务端生成并按会话密钥加密下发（§3.8 L4） |

> **明确不做的部分**：计量统计、账单、支付网关、自动扣款、**试用 trial**。**首版 Binance + OKX** 两家交易所。

### 4.2 核心流程（管理侧）

```
注册登录 → 新建组织(自动 Owner)
     ↓
管理员增加用户（账号密码）→ 创建团队 → 增加团队人员（指定 Lead）
     ↓
管理员给团队分配功能授权（自动展开隐含前置、实盘类需二次勾选）
     ↓
成员在官网申请 key（明文仅展示一次）→ 粘贴到 QuantDSH 客户端
```

### 4.3 数据模型

沿用已定稿的六表：**`orgs` / `users` / `teams` / `team_memberships` / `feature_grants` / `access_keys`**，
外加审计表 `audit_log`（建议 P0 一并落地，因为授权一旦可改就必须可追溯）。

适用性调整（v1.1）：`feature_grants` 现在要同时承载**个人订阅**与**团队订阅**两种权益主体，
因此增加 `subject_type / subject_id` 取代原先只绑 team 的形态，并新增订阅与席位两张表。

约束调整（v1.2 · **单组织**）：**一个用户只属于一个组织**，永不出现跨组织成员关系。
这条简化很值钱——授权求解不必做组织上下文切换，客户端 `entitlement` 快照里 `org` 是标量而非数组，
`"$APPDATA%\WorkDSH\profiles\workdsh"` 下的数据命名空间也只需按 `org:user` 一级区分。

```sql
-- 用户：org_id 非空且为单值，一个用户永不属于多个组织
CREATE TABLE users (
  id            TEXT PRIMARY KEY,              -- qu_<uuid>
  org_id        TEXT NOT NULL REFERENCES orgs(id),
  email         TEXT NOT NULL UNIQUE,          -- 单组织前提下，邮箱可全局唯一
  display_name  TEXT NOT NULL,
  role          TEXT NOT NULL DEFAULT 'MEMBER', -- OWNER|ADMIN|MEMBER
  status        TEXT NOT NULL DEFAULT 'active', -- active|disabled
  password_hash TEXT,                          -- 后台 SSO 账号可为 NULL
  created_at    INTEGER NOT NULL
);
CREATE INDEX idx_users_org ON users(org_id);

-- 若未来确需多组织，本表不动，改为新增 user_org_memberships 关联表；届时客户端
-- entitlement 的 org 字段需从标量升级为列表并重做全链路，故首版明确锁定单组织。
```

```sql
-- 权益（个人订阅 → subject_type='user'；团队订阅 → subject_type='team'）
CREATE TABLE feature_grants (
  id            TEXT PRIMARY KEY,             -- qg_<uuid>
  org_id        TEXT NOT NULL REFERENCES orgs(id),
  subject_type  TEXT NOT NULL,                -- user|team
  subject_id    TEXT NOT NULL,                -- users.id | teams.id
  feature_key   TEXT NOT NULL,                -- 见终版 §3.3 的 18 项
  access_level  TEXT NOT NULL,                -- view|use|manage
  source        TEXT NOT NULL DEFAULT 'manual', -- manual|plan|grant（**无 trial**，v1.3 决策 C）
  granted_by    TEXT NOT NULL,
  granted_at    INTEGER NOT NULL,
  expires_at    INTEGER,                      -- NULL = 长期；到点即失效（§3.5.1）
  revoked_at    INTEGER,                      -- 撤销时刻，参与权益结算
  UNIQUE(subject_type, subject_id, feature_key)
);
CREATE INDEX idx_grants_team ON feature_grants(subject_type, subject_id);

CREATE TABLE audit_log (
  id         TEXT PRIMARY KEY,      -- qa_<uuid>
  org_id     TEXT NOT NULL REFERENCES orgs(id),
  actor_id   TEXT NOT NULL,
  action     TEXT NOT NULL,         -- grant.add|grant.revoke|user.disable|key.issue|key.revoke
                                    -- |seat.assign|seat.release|sub.create|live.enable|...
  target     TEXT NOT NULL,         -- team_id|user_id|key_id|feature_key
  detail     TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_audit_org_time ON audit_log(org_id, created_at DESC);

-- 订阅：记录权益来源与有效期；**不做按次计量**，但支持套餐与支付
CREATE TABLE subscriptions (
  id           TEXT PRIMARY KEY,              -- qs_<uuid>
  org_id       TEXT NOT NULL REFERENCES orgs(id),
  subject_type TEXT NOT NULL,                 -- user|team
  subject_id   TEXT NOT NULL,
  plan_key     TEXT NOT NULL,                 -- pro|team-...（映射到一组 feature_key）
  seat_limit   INTEGER,                       -- 团队订阅才有；NULL 表示不限/个人
  status       TEXT NOT NULL DEFAULT 'active', -- active|expired|cancelled
  source       TEXT NOT NULL DEFAULT 'manual', -- manual|purchase|grant（**无 trial**）
  auto_renew   INTEGER NOT NULL DEFAULT 0,    -- 是否到期自动续订
  starts_at    INTEGER NOT NULL,
  expires_at   INTEGER,                       -- NULL = 长期有效；到点即失效
  cancelled_at INTEGER,
  symbol_scope TEXT                           -- 覆盖 plan 的默认额度（JSON，见下）
);

-- 套餐：一组 feature_key 的定价包装（v1.2 新增，对齐参考项目的 plan 模型）
CREATE TABLE plans (
  code        TEXT PRIMARY KEY,               -- free|pro|team
  name        TEXT NOT NULL,
  price_cents INTEGER NOT NULL DEFAULT 0,
  currency    TEXT NOT NULL DEFAULT 'CNY',
  interval    TEXT NOT NULL DEFAULT 'month',  -- month|year
  feature_keys TEXT NOT NULL,                 -- 逗号分隔的 feature_key 列表
  seat_included INTEGER,                      -- 含席位数；NULL = 个人套餐
  symbol_scope TEXT NOT NULL DEFAULT '{}',    -- 额度：{"mode":"whitelist"|"all","symbols":[...]}（§3.7）
  is_active   INTEGER NOT NULL DEFAULT 1
);

-- 支付订单与回调幂等（v1.2 新增）
CREATE TABLE payment_orders (
  id          TEXT PRIMARY KEY,               -- qo_<uuid>
  org_id      TEXT NOT NULL REFERENCES orgs(id),
  user_id     TEXT NOT NULL REFERENCES users(id),
  plan_code   TEXT NOT NULL REFERENCES plans(code),
  amount_cents INTEGER NOT NULL,
  status      TEXT NOT NULL DEFAULT 'pending', -- pending|succeeded|failed|refunded
  pay_url     TEXT,
  trade_no    TEXT UNIQUE,                    -- 支付网关流水号，用于回调幂等
  raw         TEXT,                           -- 回调原文，便于对账
  created_at  INTEGER NOT NULL,
  paid_at     INTEGER
);
CREATE INDEX idx_orders_org ON payment_orders(org_id, created_at DESC);

-- 席位：团队成员占用团队订阅的一个位置，占用后才继承团队权益
CREATE TABLE seats (
  subscription_id TEXT NOT NULL REFERENCES subscriptions(id),
  user_id         TEXT NOT NULL REFERENCES users(id),
  assigned_by     TEXT NOT NULL,
  assigned_at     INTEGER NOT NULL,
  released_at     INTEGER,
  expires_at      INTEGER,                    -- 席位到期即失效（继承 subscription 或单独设定）
  PRIMARY KEY (subscription_id, user_id)
);
```

```sql
-- ===== v1.3 新增：授权完整性支撑表（§3.8） =====

-- 吊销增量：客户端启动时拉取 since 之后的变更，低延迟传播吊销
CREATE TABLE revocations (
  version     INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL,                 -- key|grant|seat|user
  target_id   TEXT NOT NULL,
  org_id      TEXT NOT NULL REFERENCES orgs(id),
  reason      TEXT NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL
);
CREATE INDEX idx_revocations_created ON revocations(created_at);

-- 设备绑定：一个 key 允许有限数量的设备；重装可解绑一次（人工或自助）
CREATE TABLE key_devices (
  key_id        TEXT NOT NULL,
  device_id     TEXT NOT NULL,               -- 客户端本地随机生成，不采集硬件指纹
  first_seen_at INTEGER NOT NULL,
  last_seen_at  INTEGER NOT NULL,
  last_ip       TEXT,
  PRIMARY KEY (key_id, device_id)
);

-- 快照签名密钥（轮换用），公钥随客户端版本下发
CREATE TABLE signing_keys (
  kid         TEXT PRIMARY KEY,              -- key id，写进 JWS 头
  public_pem  TEXT NOT NULL,
  private_pem TEXT NOT NULL,                 -- 生产环境应存 KMS/密文，不落明文
  status      TEXT NOT NULL DEFAULT 'active', -- active|retired
  created_at  INTEGER NOT NULL
);

-- 高风险操作票据审计（实盘/自动下单，见 §3.8 L5）
CREATE TABLE execution_tickets (
  id          TEXT PRIMARY KEY,              -- qt_<uuid>
  user_id     TEXT NOT NULL REFERENCES users(id),
  feature_key TEXT NOT NULL,                 -- trade.live|trade.auto-exec
  issued_at   INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,              -- 建议 10–30 分钟
  device_id   TEXT NOT NULL,
  consumed_at INTEGER
);
CREATE INDEX idx_tickets_user ON execution_tickets(user_id, issued_at DESC);
```

> **`symbol_scope` 必须放服务端下发**，不得硬编码进客户端；否则调整币种名单就要发新版（§3.7）。

> 版本差异：`QUANTDSH_ENTERPRISE_PLAN.md` §10.2 的旧 DDL（`feature_grants` 仅 `team_id`）**已被本节取代**；其余五表不变。

### 4.4 API 契约（汇总版）

```
【官网 · Bearer <session>】
POST /api/auth/register|login|refresh        GET /api/me
GET  /api/orgs            POST /api/orgs
POST /api/orgs/:id/users                     [owner|admin]
POST /api/orgs/:id/teams                     [owner|admin]
POST /api/teams/:id/members                  [owner|admin|lead]
POST /api/teams/:id/grants                   [owner|admin]
GET  /api/orgs/:id/audit                     [owner|admin]
POST /api/keys/issue      POST /api/keys/revoke

【套餐与支付 · 有商业闭环，但**无按次计量**】
GET  /api/plans                    公开套餐列表与各自 feature_key
POST /api/orders                   下单 → { order_id, plan_code, amount, pay_url }   [本人]
GET  /api/orders/:id               查询订单状态                                       [本人]
POST /api/pay/notify               支付异步回调（**幂等**：同 trade_no 重复回调只生效一次）
GET  /api/orders/:id/return        同步跳转回官网成功页（带 order_id）

【订阅与席位 · 权益判定】
GET  /api/subscriptions            列出我可见的个人/团队订阅          [owner|admin|本人]
POST /api/subscriptions            为用户或团队建立订阅权益来源        [owner|admin]
POST /api/subscriptions/:id/seats  分配/释放团队成员席位              [owner|admin|lead]
GET  /api/users/:id/entitlements   该用户的权益明细（管理端排障用）    [owner|admin]

【客户端 · Bearer <key>】
POST /api/key/verify      → {
                             // JWS 签名：alg=EdDSA, kid=<signing_keys.kid>
                             user, org, teams[],
                             grants[{feature_key, access_level, expires_at, source}],
                             symbol_scope{mode, symbols},        // §3.7
                             device_id, iat, exp, version,       // exp = 快照 TTL（建议 7 天）
                             must_refresh_before, server_time    // §3.8 L3
                           }
GET  /api/entitlements     → 刷新快照（同上口径，服务端按 grant / subscription / seat 实时结算）
GET  /api/revocations?since=<version>  → 吊销增量（启动必调，可失败降级）
POST /api/key/device-bind  → 设备绑定/解绑（重装自助解绑一次）
GET  /api/releases/latest  → 版本清单 + 签名与哈希（§3.8 L6）

【敏感操作票据 · 实盘类必须在线】
POST /api/tickets/issue    → { feature_key, device_id } → { ticket, expires_at }   [需对应权益]
                              # 写 execution_tickets + audit_log；票据过期后需续取
                              # 已持仓的风控/止损本地仍可执行，不受票据过期影响（可用性红线）

【协同（见优先级）】
GET|POST /api/agent/memory       记忆检索与写入        [P1]
POST     /api/agent/reflect      反思回写（自动/手动）  [P1]
GET|POST /api/teams/:id/assets   团队共享区读写        [P1, 写需 manage]
POST     /api/market/publish     发布到市场            [P2, 需 manage]
```

### 4.5 结构化 Token

```
org.user.<32 字节随机 hex>
```
服务端只存 `sha256(plaintext)`；明文仅在 issue 响应中出现一次。前缀便于日志路由与误报排查，**不承载任何权限语义**
（权限永远以服务端按 `feature_grants` + `seats` 结算的结果为准）。

**四种用法都指向同一枚 personal key**：同一用户在不同机器上用同一枚 key；换机器/重装不丢权益（权益在服务端，不在本地）。
团队订阅不发行"团队公共 key"——成员各用自己的 key 登录，靠**席位**继承团队权益，以便追责与吊销。

---

## 5. 桌面客户端（QuantDSH）

### 5.1 版本形态：免费默认 + 订阅解锁（在个人版基础上发展）

> **决策（v1.1）**：只有一个客户端包。装好即免费可用；想用订阅能力才需要一枚 key。
> 不再划分"个人版 / 企业版"两个发行物——版本迷宫对用户无价值，对我们要维护两套分支更糟。

| | **免费（默认）** | **订阅 · 个人 key** | **订阅 · 团队席位** |
|---|---|---|---|
| 如何获得 | 安装即用 | 官网/平台获取一枚个人 key | 成员被分配团队席位 |
| 权益主体 | 客户端内置白名单（切法待定） | user | team（经席位传递） |
| 可用功能 | 免费功能键（§3.3，**划分后补**） | 个人订阅覆盖的能力 | 团队订阅覆盖的能力 |
| **可用标的** | 内置 `FREE_SYMBOLS` 若干主流币种（离线永久） | `symbol_scope` 放开（快照下发、签名保护） | 同左 |
| 离线 | **永久可用** | 签名快照内各权益自带 `expires_at`；**到点即失效** | 同左 |
| 共享区 | 无 | 无（除非也在团队里） | ✅ `team.collab` |
| 实盘 | 无 | `trade.live`（Preflight + opt-in + **在线票据**） | 同左 |
| 换机/重装 | 不受影响 | 用同一枚 key 恢复权益（自助解绑一次） | 席位重分配后恢复 |

> 这条设计的关键收益：**用户侧"先跑起来"零门槛，商业侧付费点全部落在生产与团队。

### 5.2 客户端分层与改造点

| 层 | 现状 | 需要的改动 |
|---|---|---|
| carrier（`app.rs`） | `PROFILE_NAME = "workdsh"` | **保持不变**（v1.1：沿用 workdsh profile 扩展） |
| `local_runtime.rs` | 两处硬编码 `"workdsh"` | 保持不变；如需常量化只做整理，不改值 |
| `enterprise_auth.rs` | 代理外部后台账号密码登录 | 改为 key 登录 + 本地 SQLite 快照；协作/插件代理保留 |
| 新增 `entitlement` 模块 | — | keychain 存 key、`entitlement.db` 存快照、`FREE_KEYS` 白名单 + `can_access` 命令 |
| profile 内新增 quant 工作区 | 无 | 数据中枢/策略工坊/交易台/智能体的 UI 与本地服务（见 §6） |
| 菜单与载体页 | 已完成 QuantDSH 文案 | 增加「量化工作台」入口组与"已订阅/未订阅"状态展示 |

### 5.3 产品 UI 地图（客户端侧导航）

```
工作台      今日概况 / 我的任务 / 最近会话 / 待处理告警
数据中枢    连接器 · 同步任务 · 数据湖 · 加密专属（资金费率/基差/清算/链上）· 本库预览
策略工坊    因子实验室 · 回测 · 成本与保证金配置 · 报告与结果工作台
交易台      账户与风控 · 仿真/实盘 · Preflight 报告 · 持仓订单 · 性能归因
投研智能体  对话入口 · Skill/Expert/Team · 流水线编排 · 记忆库 · 决策留痕
团队与资产  共享区 · 我的发布 · 可复现包
设置        身份与授权（key 状态、团队、已开功能）· 数据目录 · Preflight 诊断 · 关于
```

**UI 与门禁的耦合规则**：未授权的导航项**显示但置灰并给出原因**（比隐藏更可理解），`trade.live` 类敏感项默认完全隐藏直到被授予。

### 5.4 本地存储布局

```
%APPDATA%\WorkDSH\profiles\workdsh\          ← 沿用既有 profile（不新建）
  quant\                                     ← 量化命名空间（与其它工作数据隔离）
    entitlement.db                           SQLite：授权快照（可从服务端重建）
      ├─ entitlement_snapshot                签名后的 JWS 原文 + 验签状态
      ├─ clock_guard                         last_seen_server_time（单调时钟护栏，§3.8 L3）
      └─ device_id / key 引用（明文 key 仅存 OS 凭据库）
    lake\                                    DuckDB + Parquet：bronze（原始不可变）/ silver（清洗对齐）
      └─ secure\                             **加密派生数据集**（§3.8 L4），密钥不落盘
    memory\                                  SQLite：本地智能体记忆（订阅用户与服务端同步）
    workspace\                               会话、报告、产物、血缘 provenance.json
    logs\                                    运行留痕、审计底账（清理列表不得删除）
```
交易所密钥 → **OS 凭据库**，不落磁盘明文。

---

## 6. 功能模块全集（含门禁键与优先级）

> 来源标记：`[QS]` QuantStudio · `[D]` QuantDinger · `[M]` QuantMind · `[O]` 自有
> **三家参照的功能、代码结构与商业模式一律完整参考**（不再因许可证做取舍，见 §8）。
> 优先级：**P0** 首版必须 / **P1** 首个完整闭环 / **P2** 生产化与生态

### 6.1 数据与底座

| 功能 | 门禁键 | 来源 | 优先级 |
|---|---|---|---|
| 沿用 `workdsh` profile 扩展 quant 工作区、独立数据命名空间、离线优先 | `core.app` | [O] | P0 |
| 多源连接器框架：`DataSource` trait + 按 market 注册 + **失败软着陆**（首版 **Binance + OKX**） | `data.hub` | [D] | P0 |
| 本地数据湖：Parquet + DuckDB，bronze→silver 分层 | `data.hub` | [M][O] | P0 |
| 增量同步、断点续传、同步任务账与指纹校验 | `data.hub` | [M][O] | P0 |
| **加密专属**：资金费率 / 期现基差 / 标记价 / 清算 / 深度 / 链上 ⭐ | `data.crypto-native` | [O] | P0 |
| 数据质量治理：缺失停牌复权、跨源校验、**缺失留空不填 0** | `data.hub` | [QS][O] | P1 |
| **PIT 时点数据 vintage**（消除前视偏差）⭐ | `data.hub` | [QS] | P1 |
| 特征注册表（元数据/版本/血缘） | `data.hub` | [M][O] | P1 |
| 舆情与事件脉、情绪量化 | `data.hub` | [M] | P2 |
| 数据漂移 PSI 检测告警 | `quality.monitor` | [M] | P2 |
| 更新与回滚（新版本独立目录验证后切换、失败自动回滚） | `core.app` | [QS] | P1 |
| 标的符号表与合约元数据（交易所/到期/乘数/tick） | `data.hub` | [D] | P0 |
| **免费额度按币种切分**：`FREE_SYMBOLS` 常量（离线）+ 服务端下发签名 `symbol_scope`；闸门在**数据入口**（连接器/同步/回测/仿真），UI 仅提示（§3.7） | `data.hub` | [O] | P0 |
| **授权完整性**：签名快照（Ed25519）+ fail-closed + 单调时钟 + 吊销增量 + 设备绑定；**判定下沉 native**（§3.8 L1–L3、L6） | `core.app` | [O] | P0 |
| **有状态任务必须单 worker**（job 状态不跨进程，否则日志/状态 404） | `core.app` | [M][D] | P0 |
| **内部回调 secret 生产环境 fail-closed**（缺失直接拒绝启动） | `core.app` | [M] | P0 |

### 6.2 研究与策略

| 功能 | 门禁键 | 来源 | 优先级 |
|---|---|---|---|
| 因子库与因子评估（IC/IR/衰减/分组/换手 + 报告） | `research.factor` | [QS][D] | P0 |
| **回测引擎（免费基线）**：事件驱动 + commission / slippage / leverage | `research.backtest` | [D] | P0 |
| **crypto 成本补完 + mtf 多周期**：资金费率、期现基差、逐仓/全仓保证金、强平结算 ⭐ | `research.backtest-pro` | [O] | P0 |
| 因子生命周期：健康矩阵、拥挤度告警、退役/重建候选、provenance、handoff_card ⭐ | `research.factor-pro` | [QS] | P1 |
| 研究血缘与一键复现（数据版本+因子+参数+代码全链路）⭐ | `research.factor-pro` | [QS][O] | P1 |
| **因子五元组建模**（名称 / 描述 / 公式 / 表达式 / 变量），让因子可复现、可评审、可进化 ⭐ | `research.factor` | [M] | P0 |
| **K 线内部路径推断**（阳线 O→L→H→C、阴线 O→H→L→C），无 tick 时判定止损/止盈谁先触发 ⭐ | `research.backtest` | [D] | P0 |
| **信号周期 × 成交周期分离**：自动选精度（≤30 天用 1m、≤1 年用 5m、更久拒绝高精度）；**高精度仅 crypto 开放** ⭐ | `research.backtest-pro` | [D] | P0 |
| 因子执行结果缓存（命中 md5(源码) 直接复用，不重跑） | `research.factor` | [M] | P1 |
| 沙箱：AST + 正则静态校验 + 子进程超时/内存限制（**Windows 不支持 signal，须用子进程方案**） | `research.factor` | [D][O] | P1 |
| 错误脱敏（绝对路径替换）+ 超长截断（头尾各保留），护隐私且控 LLM token | `research.agents` | [M] | P1 |
| 组合优化与风控（约束优化、Sharpe/Sortino/回撤/β） | `research.backtest` | [O][D] | P1 |
| 结果工作台（报告/表格/代码并排核对、AI 建议参数调整） | `research.backtest` | [QS][D] | P1 |
| 跨资产规则插件位（涨跌停/停牌/整手/T+1，为 A 股期货预留） | `research.backtest` | [M] | P2 |
| **因子自主进化**（假设→公式合成→遗传演化→入库）⭐ | `research.automl` | [M] | P2 |
| 模型工场与集成（`ModelAdapter` 接口） | `research.automl` | [M] | P2 |
| **生产质量闭环**：Rank IC/ICIR 回填 + SHAP + PSI 双通道告警 ⭐ | `quality.monitor` | [M] | P2 |

### 6.3 交易执行

| 功能 | 门禁键 | 来源 | 优先级 |
|---|---|---|---|
| **券商/交易所适配层**：`BrokerAdapter` + factory + 内置 endpoint + spot/perp 分支；**首版 Binance + OKX** ⭐ | `trade.paper` | [D] | P0 |
| **Preflight Check**：凭据/余额/网络/时钟/合约/额度/参数自检，不过不启动 ⭐ | `trade.paper` | [M] | P0 |
| 仿真交易（testnet / 纸面，接口与实盘同构） | `trade.paper` | [O] | P0 |
| 实盘交易（用户自有 key，**默认关闭 + 显式 opt-in**，可地区/构建关闭） | `trade.live` | [O] | P1 |
| 执行方式：逐笔确认 或 授权后自动；**暂停语义**（停新委托，不撤已提交、不自动平仓） | `trade.live` | [QS] | P1 |
| **成交与费用口径**：平仓净盈亏 = 平仓毛利 − 开仓费 − 平仓费；分批按手数分摊；跨日保成本；缺费"待核算"⭐ | `analysis.performance` | [QS] | P1 |
| **性能归因**：净盈亏曲线、归因拆解、毛/净显式标注、采样缺口检查、CSV 导出、缺失留空 ⭐ | `analysis.performance` | [QS] | P1 |
| 订单/持仓全生命周期、线程池 + 订单队列 + **重启自动恢复运行中策略** | `trade.paper` | [D] | P1 |
| 限流治理（查询/交易分类限速 + 退避重试） | `trade.paper` | [QS] | P1 |
| AI 作为交易过滤器（风险不一致则禁止下单，策略开关） | `research.agents` | [D] | P2 |
| 自动下单（免逐笔确认） | `trade.auto-exec` | [O] | P2 |

### 6.4 投研智能体

| 功能 | 门禁键 | 来源 | 优先级 |
|---|---|---|---|
| **三级能力模型**：Skill → Expert → Expert Team（负责人+成员+任务依赖）⭐ | `research.agents` | [QS] | P0 |
| 对话式总入口（自然语言驱动整条流水线） | `research.agents` | [O][M] | P0 |
| 技能包与产物归档（研报 MD/PDF 入档案库） | `research.agents` | [M][QS] | P1 |
| **多智能体流水线**：分析师 → 多空辩论 → 交易员 → 风险三方辩论 → 终决 ⭐ | `research.agents-pro` | [D] | P1 |
| 结构化决策输出（recommendation/confidence/trading_plan/reasoning），可审计留痕 | `research.agents` | [D] | P1 |
| **记忆增强 + 反思闭环** ⭐：本地 SQLite + 打分 `0.75·sim + 0.20·recency + 0.05·tanh(returns) − tf惩罚`（半衰期 30 天）+ 本地哈希向量 + 自动反思 worker + 手动复盘 | `research.agents-pro` | [D] | P1 |
| 记忆集中在服务端共享（跨会话/跨成员）+ 客户端只读缓存 | `research.agents-pro` | [O] | P2 |
| 加密分析师扩展（链上 / 资金费率结构 / 清算地图）⭐ | `research.agents-pro` | [O] | P2 |
| LLM 结构化输出（强制 JSON）+ 主备降级链 + **HTTP 402 自动降级**，避免 AI 调用一失败就中断 | `research.agents` | [D] | P0 |
| **成功/失败知识复用 + 超量抽样 + 并行实现**（已解过的不重算、失败过的不重试、超量则抽样） | `research.agents-pro` | [M] | P2 |
| **研发闭环** propose→exp→code→run→feedback→累积 Trace | `research.automl` | [M] | P2 |
| Scenario 可插拔研发框架（因子 / 模型 / 数据科学共用一套演化器，流水线不写死） | `research.automl` | [M] | P2 |

### 6.5 协作、资产与组织

| 功能 | 门禁键 | 来源 | 优先级 |
|---|---|---|---|
| 会话生命周期（继续/归档/恢复；删除保留生成文件；完整删除二次确认） | `core.app` | [QS] | P1 |
| **团队共享区**（共享策略/因子/报告，写需 manage）⭐ | `team.collab` | [O] | P1 |
| 官网组织/用户/团队/成员管理 + 审计 | — | [O] | P1 |
| **单组织约束**：一个用户只属于一个组织（`users.org_id` 单值，email 全局唯一） | — | [O] | P0 |
| API Key 生命周期：明文**仅返回一次** + 可轮换 + 可吊销 + 「确保有默认 key」的低门槛初始化 | `core.app` | [M] | P0 |
| **订阅套餐 + 支付闭环**：套餐 = 一组 feature_key；下单 → 支付 → 异步回调**幂等** → 激活订阅 | — 官网 | [M] | P1 |
| 注册礼包 + 邀请同组织成员（赠**权益有效期**，不引入积分） | — 官网 | [D] | P2 |
| **功能挂团队 + key 登录 + 离线门禁** ⭐ | — | [O] | P1 |
| 可复现发布（带 provenance 的研究包）⭐ | `market.publish` | [QS][O] | P2 |
| 策略/因子市场（指标展示 + 一键导入） | `market.publish` | [QS][M][O] | P2 |
| Work 办公（表格/文档/演示汇报） | `core.app` | [QS] | P2 |
| 团队告警推送（应用内/邮件/Webhook；IM 需先补鉴权） | `notify.alerts` | [M] | P2 |

---

## 7. 关键端到端链路

| # | 链路 | 验证口径（完成时怎么算完成） |
|---|---|---|
| 1 | **新人入职拿到工具**：官网建用户 → 加入团队 → 申请 key → 客户端登录 → 功能按团队出现 | 换一台机器重放同一枚 key，功能集合完全一致；离线期间按签名快照工作，权益到期即刻消失 |
| 2 | **数据落地**：交易所公开 API → bronze Parquet → silver 对齐 → DuckDB 查询 → 含资金费率/基差 | 断网时历史数据仍可查询；缺失区间显示为空而非 0 |
| 3 | **研究到决策**：假设 → 因子 → 回测（crypto 成本全项）→ 报告 → 纳入/淘汰并记录 provenance | 同一研究的参数与数据版本可被他人一键复现 |
| 4 | **交易执行**：策略 → Preflight → 仿真 → （opt-in）实盘 → 成交回写 → 归因 | Preflight 任一项不通过则拒绝启动；暂停语义不误平已有仓位 |
| 5 | **智能体积累**：分析前检索记忆 → 产出决策 → 事后验证 → 写回 → 下次更准 | 同一标的第二次分析引用了上次经验；半年后可见 hit-rate 提升 |
| 6 | **组织审计**：任何授权变更产生不可删 audit 记录 | 管理员可回答"某人在某天为什么能用实盘" |
| 7 | **失效与防篡改**：篡改/回放快照 · 权益到期 · 席位释放 · key 吊销 | 改任一个字节的快照即 fail-closed 到免费档；到期的权益离线也失效；把系统时间调回过期前仍无法启用敏感项 |

---

## 8. 参考借鉴策略（许可证仅作事实记录，不作借鉴门禁）

> **决策（v1.2）：按你的指示，调研不设开源许可门禁。** 三家参照的**功能全集、代码级实现、商业模式**一律完整参考与借鉴；
> 许可证仅在 §8.3 登记事实以备日后法务核查，**不作为"能不能借鉴"的前置条件**。
> 完整调研证据见 `QUANTDSH_REFERENCE_RESEARCH.md`（v2）。

### 8.1 我们各自从谁身上拿什么（代码级）

| 参照 | 主要借鉴资产 | 关键源码位置 | 落到我们的哪一节 |
|---|---|---|---|
| **QuantDinger** | 数据源抽象与失败软着陆；9 家 crypto 交易所 + IBKR/MT5 适配工厂；**K 线内部路径推断**；**MTF 信号/成交周期分离**；LLM 结构化输出与 402 降级；代码沙箱；积分+VIP 双轨与增长回路 | `data_sources/base.py`、`live_trading/{base,factory}.py`、`services/backtest.py`、`services/llm.py`、`utils/safe_exec.py`、`services/billing_service.py` | §6.1 数据中枢、§6.2 研究与策略、§6.3 交易执行、§1.4.1 商业化 |
| **QuantMind** | **因子五元组建模**；因子执行结果 md5 缓存；错误脱敏与智能截断；Scenario 可插拔研发框架；成功/失败知识复用；**订阅套餐 + 支付宝幂等回调 + 多租户 + API Key 轮换**；社区 UGC | `coder/factor_coder/factor.py`、`core/scenario.py`、`CoSTEER/evolving_strategy.py`、`user_app/{subscriptions,api_keys}`、`routers/community` | §6.2 因子实验室、§6.4 投研智能体、§1.4.1 商业化、§4 官网服务端 |
| **QuantStudio** | **PIT 时点数据**、**严格费用口径**（毛/净标注、缺费待核算、采样缺口）、组合守护、Skill/Expert/Expert Team 三级模型、benchmark 诚实评估纪律 | 技能库 `a-share-pit-fundamental-vintage-builder`、`alpha-portfolio-guardian` | §6.2 因子实验室、§6.5 协作与资产 |

### 8.2 借鉴来的具体工程手法（已采纳进功能集）

| 手法 | 出处 | 价值 |
|---|---|---|
| 阳线 O→L→H→C / 阴线 O→H→L→C 的**K 线内部路径推断** | QuantDinger `_infer_candle_path` | 无 tick 数据也能近似判定止损/止盈谁先触发 |
| **信号周期 × 成交周期分离**（>30 天自动降 5m，>1 年拒绝高精度，仅 crypto 开放） | QuantDinger `MTF_CONFIG` | 长周期信号也能精细撮合，且自带资源护栏 |
| **因子 = 名称+描述+公式+表达式+变量**五元组 | QuantMind `FactorTask` | 因子可复现、可评审、可进化 |
| **缓存键 = md5(源码)** 的结果缓存 | QuantMind `cache_with_pickle` | 同码不重跑，迭代因子省算力 |
| 错误信息路径脱敏 + 2000 字符头尾截断 | QuantMind `FactorFBWorkspace` | 护隐私、控 LLM token |
| LLM 输出 `json_object` + 主备降级链 + **HTTP 402 自动降级** | QuantDinger `llm.py` | AI 调用健壮性 |
| **有状态任务必须单 worker** | 两家共同教训 | 避免 job 状态跨进程导致 404 |
| **内部回调 secret 生产环境 fail-closed** | QuantMind `INTERNAL_CALL_SECRET` | 防伪造回调 |
| Windows 上 `signal` 超时无效 → **须用子进程方案做沙箱** | QuantDinger `safe_exec.py` 的踩坑 | 我们是 Windows 桌面端，必须避开此坑 |

### 8.3 许可证事实记录（备查，非门禁）

| 参照 | 许可证事实 | 备注 |
|---|---|---|
| QuantDinger | **Apache 2.0**（代码）；名称/Logo/品牌展示需单独商业授权 | README 有"商业授权与赞助"章节 |
| QuantMind | **AGPL v3**；代码内标注 **OSS Edition** | AGPL 第 13 条含网络交互触发条款，若未来对外提供服务需留意 |
| QuantStudio | **GPL-3.0-or-later** / **PandaAI 商业授权**双轨；技能包含独立 LICENSE | README 明示双授权 |

> 以上仅为事实登记。若将来出现对外分发、商标使用或客户尽调需求，再回来评估来源与授权事项。

---

## 9. 差异化（三家都没有或没做好的）

1. **crypto 原生金融工程** —— 资金费率、期现基差、逐仓/全仓保证金、强平结算、链上数据。三家均未建模。
2. **桌面单实例 + 本地优先** —— QS 是 Web 工作台，Dinger 是 Flask+Vue 自托管，Mind 是 Docker+Electron。
3. **企业团队 + 功能授权 + 数据隔离** —— 三家皆无团队级授权；QS 更明确"共享部署无成员隔离"。
4. **会积累经验的投研智能体** —— 记忆反思闭环放在 crypto 场景，并可由服务端跨会话共享。
5. **数据自有** —— 不绑 PandaData / QuantDB。
6. **研究严谨性** —— QS 的 PIT 与严格费用口径（毛/净标注、缺费待核算、采样缺口检查、缺失留空）+ Mind 的生产质量闭环。
7. **按币种计的额度模型** —— 免费/收费的边界落在"可研究哪些标的"，而不是截短历史或限速；样本始终完整，研究结论始终成立（§3.7）。
8. **可在开源客户端上交付权益体系** —— 用"价值放服务端 + 有效期写在签发物 + native 判定 + 派生数据加密"替代客户端 DRM，
   做到既不伤付费用户、又能把篡改成本抬到订阅价之上（§3.8）。这三者组合在参照项目里都不存在。

---

## 10. 路线图

> 已确认的启动方式：**契约优先**（先定稿 API / token / 表结构，再并行实现两端）。
> 决策已并入下表：`workdsh` profile 扩展 · 免费 + 订阅分层 · 服务端托管共享区 · Binance + OKX · **一个用户单组织** · 不做按次计量 · 不处理 license · **免费额度按币种切** · **无 trial** · **席位到期立即失效** · **防破解设计（代码公开）**。

| 阶段 | 目标 | 交付物 | 完成判定（DoD） |
|---|---|---|---|
| **P0 · 契约与骨架** | 两端有共同语言；**免费闭环能离线跑通** | 契约定稿（§3.3 的 18 项 key + §4.3 DDL + §4.4 路由）；服务端最小集（账号/组织/用户/团队/授权/席位/Key/verify/审计/**吊销增量/设备绑定/签名密钥**）；（`users.org_id` 单值、email 全局唯一）；客户端 key 登录 + SQLite 快照 + `FREE_KEYS` + `can_access` 单测；**授权完整性基线（§3.8 L1–L3、L6）**：Ed25519 验签 + fail-closed + 单调时钟 + 判定下沉 native；**额度按币种**（§3.7）：`FREE_SYMBOLS` + 服务端下发 `symbol_scope`，闸门在连接器/同步/回测/仿真入口；profile 内 quant 工作区与 `quant\` 命名空间；Data Hub **Binance + OKX**（基础行情 → DuckDB）；基础回测（费/滑/杠杆 + **K 线内部路径推断**）；因子评估（**五元组建模**）；Preflight + 仿真；Skill/Expert/Team 与对话入口 | 不登录、断网状态下仍能跑通"取数→回测→仿真"；持有 key 时订阅功能出现；`can_access` 单测覆盖隐含前置、**权益到期逐秒失效**、席位释放、快照过期；**篡改快照任一字节即 fail-closed**；系统时间回拨无法让过期权益复活；**同一邮箱无法被第二个组织再拉入** |
| **P1 · 订阅闭环与生产投研** | 从"能研究"到"研究可积累、订阅可交付" | **官网套餐与支付闭环**（`/plans`、`/orders`、幂等回调 → 激活订阅、`auto_renew`）；API Key 轮换/吊销/默认初始化；实盘 opt-in（**仅限 Binance + OKX**，Preflight 守门 + **在线票据 §3.8 L5**）；`research.*-pro` 三件套（crypto 成本补完+mtf、因子生命周期血缘、多智能体流水线 + 记忆反思含服务端两 API）；`data.crypto-native`；**服务端供给型数据的加密下发（§3.8 L4）**；严格费用口径与性能归因；PIT 时点数据；**服务端托管的团队共享区** + 席位管理；（v1.1 明确）**订阅权益判定不含计量与账单** | 换一枚已占席的 key 后功能集合正确变化；释放席位后下次刷新权益被裁剪；**到期时刻到点的前后各一次调用结果正确**；同一研究可被他人一键复现；实盘只能经 Preflight + 显式开关 + 有效票据进入 |
| **P2 · 生产化与生态** | 长期有效性、多资产、生态 | 统一做**免费 / 订阅的功能切分与套餐定价**（v1.3 决策 B）；质量闭环（IC/SHAP/PSI）；因子自主进化；研发闭环 Trace；模型工场；多资产扩展（A 股/期货/美股 + IBKR/MT5 形态）；策略与因子市场 + 可复现发布；Work 办公；团队告警 | 因子上线后有持续有效性监控与漂移告警；新资产类别可在不强改内核前提下接入；**切分上线只改套餐配置与一处常量，功能代码零改动** |

> **不在本路线图内**：按次计量统计与用量账单、license 合规工作、**试用期机制（不做 trial）**。
> （注：**订阅套餐与支付闭环在范围内**，见 §1.4.1；这里排除的是"用多少算多少钱"的计量计费模型。）

---

## 11. 风险与合规清单

| # | 风险 | 级别 | 处置 |
|---|---|---|---|
| 1 | **crypto 实盘合规** | 高 | 实盘默认关闭 + 显式 opt-in + Preflight 强制；首版只放行 Binance + OKX；可按地区/构建整体关闭 |
| 2 | **凭据与数据外泄** | 高 | 交易所密钥仅存 OS 凭据库；服务端绝不接触密钥与策略源码；共享区发布物需显式确认不含敏感内容 |
| 3 | **订阅权益误判** | 中 | 权益判定单一出口：`can_access` = 免费白名单 ∪ 服务端签名快照；服务端/客户端两侧各留一组回归用例（席位释放、权益到期、key 吊销、快照过期） |
| 4 | **免费功能被误锁** | 中 | 免费项走客户端内置白名单，**不依赖任何快照与网络**；离线自测必须包含"无 key 启动"场景 |
| 5 | **IM 推送安全** | 中 | 远期若做 IM，须先补鉴权、源 IP 限制与凭据隔离，不可照搬"免登录对外端口" |
| 6 | **运行时契约被破坏** | 中 | `workdsh://`、`workdsh.localhost`、`workdsh-runtime`、`workdsh-*`、`runtime_pack::PRODUCT="WorkDSH"`、`%APPDATA%\WorkDSH` 一律不动 |
| 7 | 第三方代码来源与商标 | 低（用户已决定暂不处理） | 见 §8；若将来对外分发或尽调需要，再回来逐项评估。实现时优先参考 QuantDinger 的**结构形态**，不使用其名称/Logo |
| 8 | **授权被伪造 / 篡改（代码公开）** | **高** | §3.8 整套措施：签名快照 fail-closed、判定下沉 native、**高价值数据放服务端并加密下发**、实盘在线票据、吊销增量与设备绑定。**且不追求"无法绕过"**——目标是篡改成本高于订阅价 |
| 9 | **防破解反噬（误伤付费用户 / 被当恶意软件）** | 中 | §3.8 公司红线：不做侵入式 DRM、不采集无关隐私；期末降级语义明确（除实盘外不许因断网禁用已付功能）；任何封禁可由管理员复核撤销 |
| 10 | **到期时序不一致** | 中 | 权益时间戳一律以服务端签发为准，`expires_at` 逐项沉淀进快照；本地只做比较不做推算；离线与在线走同一函数 |
| 11 | **签名私钥泄露** | 中 | `signing_keys` 支持 `kid` 轮换；私钥加密存储/托管，轮换期双密钥并存；泄露后按 `retired` 处理并下发新公钥版本 |

---

## 12. 决策台账状态

### 12.1 第二轮已拍板（v1.1，六项，已并入全文）

| # | 议题 | 结论 |
|---|---|---|
| 1 | 开源许可证 | **暂不处理**（不作为排期门禁；见 §8） |
| 2 | 计量 / 计费 | **不做按次计量与用量账单**；但**订阅套餐 + 支付闭环要做**（见 §1.4.1） |
| 3 | 团队共享区 | **服务端托管**（§2.3 / §4.1） |
| 4 | 商业模式 | **个人版基础上发展**：免费起步 + 订阅解锁；订阅分**个人订阅**与**团队席位订阅**；**具体切法延后统一规划**（§3.5 / §5.1 / §12.2） |
| 5 | 首版交易所 | **Binance + OKX**（§6.3 / §10） |
| 6 | 技术基座 | **沿用 `workdsh` profile 扩展 quant 工作区**，不新建 profile、不改 `PROFILE_NAME`（§2.1 / §5.2） |

### 12.2 第四轮已拍板（v1.3，已并入全文）— **进入开发前的待定项已清空**

| # | 议题 | 结论 | 落点 |
|---|---|---|---|
| 1 | **免费 / 订阅的功能切法** | **本期不定**。先把功能做完整，**做完后统一规划**切法与套餐 | §3.3 层级标注撤销；§10 P2 增加"统一切分"条目；实现要求：切分上线只改服务端配置与一处白名单常量 |
| 2 | **免费版数据额度** | **按币种切**（symbol 白名单），**不按历史长度 / 不限速** | §3.7；`plans.symbol_scope` + `subscriptions.symbol_scope` 覆盖；闸门在数据入口而非 UI |
| 3 | **试用（trial）** | **不做**，不保留任何 trial 字段与逻辑 | §3.5.3；`source` 取值 `manual\|purchase\|grant` |
| 4 | **席位到期** | **立即失效**（精度到秒，离线同样成立） | §3.5.1 / §3.5.2 / §3.6 / §5.1；权益逐项自带 `expires_at`；释放与吊销为"下次联网即生效" |
| 5 | **防破解（代码公开）** | 按"客户端可被任意篡改"设计：**价值放服务端 + 有效期写在签发物 + 判定下沉 native + 高价值数据加密 + 实盘在线票据**；**不做侵入式 DRM** | §3.8（L0–L8 与三条公司红线）；§4.3 新增 `revocations`/`key_devices`/`signing_keys`/`execution_tickets`；§4.4 新增 `get revocations`、`tickets/issue` |

### 12.3 第三轮已拍板（v1.2，已并入全文）

| # | 议题 | 结论 | 落点 |
|---|---|---|---|
| A | **一个用户能否同时在多个组织** | **不能。一个用户只属于单组织**（原第 5 项待定就此关闭） | `users.org_id` 非空单值 + email 全局唯一（§4.3）；客户端 entitlement 的 `org` 为标量 |

---

> **§12.2 已无待定项**，设计阶段的开放问题全部关闭。
> 可即按 §10 的 P0 开工：**契约 → 客户端 key 登录 + `can_access`（native 判定）+ 免费白名单 + 签名快照 → 服务端最小集（含吊销/设备/签名密钥）→ 免费额度按币种 → quant 工作区（Binance/OKX → DuckDB → 基础回测 → 仿真）**。
> 唯一延后到 P2 的是**免费 / 订阅的功能切分**——按你的决定，等功能做完整后再统一规划。
