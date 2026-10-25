# QuantDSH 企业版功能设计（用户 / 团队 / 功能挂钩 · Client–Server 架构）

> **范围声明**：本文档只做功能与架构设计，**不涉及任何代码改动**。前文已确认四项决策，本版据此重写为"官网/平台（服务端）+ 本地客户端"模型。
>
> **已确认决策**：
> 1. 持久化：服务端 **SQLite**；客户端授权缓存 **SQLite**（小而纯本地，与量化数据湖 DuckDB 解耦）。
> 2. 鉴权：**新建服务端**，账号密码登录（服务端），本地客户端用 **key** 登录。
> 3. 组织粒度：用户在**官网新建组织、加入人员**，每人到平台**申请 key**，在**本地客户端用 key 登录**。
> 4. 管理 UI：组织的管理界面在**官网**里，不在客户端。
> 5. 本轮落点：**客户端 + 服务端一起交付**（新建独立官网/平台服务 + 客户端 carrier 改造）。
> 6. 服务端归属：**新建独立服务**（独立仓库/技术栈），与现有 workdsh 后台解耦。
> 7. 离线强制：**本地优先**，无网时按缓存授权做功能门禁，联网后再刷新校验。
> 8. key 形态：**结构化 token**（带组织/用户前缀，如 `org.user.xxxx`）。
> 9. 服务端技术栈：**Rust + Axum + SQLite**（独立仓库，与 carrier 同语言，便于复用依赖与部署经验）。
> 10. 启动方式：**契约优先**（先定稿两端共享契约：API schema + token 字段 + 表结构，再并行实现）。

---

## 1. 目标与本轮范围

企业版组织管理能力，明确为三项 + 一个核心约束：

1. **用户增加** —— 在官网创建组织成员账号（账号密码）。
2. **创建团队** —— 组织内建团队，用于协作与功能归集。
3. **增加团队人员** —— 把成员加入团队并区分团队内角色。
4. **相关功能和团队挂上钩** —— 功能授权以**团队**为单位；成员通过其团队隶属间接获得功能。

架构上分为两个独立组件：**官网/平台（服务端，持有目录与授权）** 与 **本地客户端（QuantDSH carrier，key 登录 + 本地强制）**。

---

## 2. 架构总览

```
┌─────────────────────────┐         key 登录 / 拉取授权         ┌──────────────────────────┐
│   官网 / 平台（服务端）   │  ◀──────────────────────────────▶  │  QuantDSH 本地客户端      │
│                          │   账号密码登录 / 管理 API           │  （Tauri carrier）        │
│  · Org / User / Team     │                                    │                          │
│  · Membership / Key 发放  │   Admin UI（官网网页）              │  · key 登录              │
│  · FeatureGrant          │   ───────────────────────────      │  · 本地缓存授权(DuckDB)  │
│  · 鉴权 API              │   用户在官网建组织/加人/发 key      │  · can_access 强制       │
│  · 持久化: SQLite（Rust+Axum） │                                    │  · 产品功能按团队授权门禁 │
└─────────────────────────┘                                    └──────────────────────────┘
```

- **服务端是目录与授权的唯一权威**（用户、团队、成员、key、功能授权都在官网）。
- **客户端不持有管理 UI**，只持有：当前登录用户的 key、从服务端拉取的"组织/团队/授权"快照、以及本地强制逻辑。
- "功能和团队挂上钩"的**判定规则在服务端定义、在客户端执行**（客户端缓存授权后离线也能门禁）。

---

## 3. 数据模型（服务端权威）

```
组织 Org
  id, name, ownerUserId, createdAt

用户 User（账号密码，服务端）
  id, orgId, displayName, email,
  passwordHash, status(active|disabled), role(Owner|Admin|Member),
  createdAt

团队 Team
  id, orgId, name, description, createdBy, createdAt

团队隶属 TeamMembership
  teamId, userId, roleInTeam(Lead|Member), joinedAt

功能授权 FeatureGrant        ← "功能和团队挂上钩"的实体（服务端定义）
  id, orgId, teamId, featureKey, featureName,
  accessLevel(view|use|manage), grantedBy, grantedAt

访问凭证 AccessKey           ← 平台发放，客户端凭此登录
  id, orgId, userId, keyHash, label,
  status(active|revoked), createdAt, lastUsedAt
```

> 客户端本地只缓存派生快照：`{ user, org, teams[], grants[] }`，持久化到 **SQLite**（对应已确认的持久化决策），用于离线强制与减少重复请求。

---

## 4. 核心流程

### 4.1 官网（管理侧，网页 UI）
1. 用户注册/登录官网 → 新建**组织**。
2. 组织管理员**增加用户**（账号密码）。
3. 管理员**创建团队**、**增加团队人员**（Membership）。
4. 管理员给团队**分配功能授权**（FeatureGrant）——即"功能挂钩团队"。
5. 成员在官网**申请 key**（平台生成并展示一次），用于客户端登录。

### 4.2 客户端（使用侧，key 登录）
1. 用户在 QuantDSH 输入 **key** 登录。
2. 客户端 `POST /api/key/verify` → 服务端校验 key，返回
   `{ user, org, teams:[...], grants:[...] }`。
3. 客户端**安全存储 key**（OS 凭据库），并把授权快照缓存进本地 DuckDB。
4. 任意产品功能在打开前调用本地 `can_access(featureKey)`：
   ```
   can_access(user, featureKey):
     取 user 的 teams → 若任一 team 在 grants 中含 featureKey
       → 返回该 team 最高 accessLevel，否则 None
   ```
5. 定期/启动时刷新快照；key 失效则强制重新登录。

---

## 5. 客户端实现点（QuantDSH carrier，暂不改）

1. **key 登录命令** `enterprise_key_login(key)`：替代现有账号密码企业登录，调用服务端校验并缓存授权。
2. **本地授权缓存**：用 **SQLite**（已确认）存 `{ user, org, teams, grants }` 与 key 引用（key 本体走 OS 凭据库，不落库明文）。
3. **强制门禁** `can_access(featureKey) -> Option<AccessLevel>`：纯本地判定，复用前文"用户→团队→功能"三级链。
4. **校验/刷新**：启动时与定时从服务端重新拉取快照；key 失效即清缓存并回到登录。
5. 与现有 `enterprise_auth.rs` 的关系：现有账号密码登录代理到外部后台的模型，改为** key 登录代理到本平台**，其余协作/插件代理逻辑保留。

## 6. 服务端实现点（官网/平台，独立组件）

1. **账号密码鉴权**：`/api/auth/register|login`（组织/用户）。
2. **组织/用户/团队/成员/授权管理 API**：CRUD + 仅管理员可操作。
3. **key 发放与校验**：`/api/key/issue`（登录用户申请）、`/api/key/verify`（客户端登录）、`/api/key/revoke`。
4. **管理 UI**：官网网页，承载第 4.1 节全部操作。
5. **持久化**：**SQLite**（Rust + Axum + SQLite，已确认）。

> 服务端为**独立于 `dsh-tauri` 仓库**的组件（技术栈已定：**Rust + Axum + SQLite**），本文档第 10 节给出契约草案。

---

## 7. 与现有企业登录的衔接

- 现有 `workdsh_enterprise_login` 走账号密码 → `/api/auth/login`；新模型改为**客户端只认 key**，账号密码仅存在于官网。
- 现有 `EnterpriseMember{ id, organizationId, role, ... }` 可复用为客户端缓存快照的字段来源（服务端 `/api/key/verify` 直接返回等价结构 + teams + grants）。
- 协作/插件代理（Authority loopback）保留，仅鉴权来源由"账号密码会话"换成"key 会话"。

---

## 8. 分阶段路线

- **阶段 1（契约对齐）**：定稿服务端 API 与客户端授权快照契约（本文件即契约）。
- **阶段 2（客户端）**：carrier 内 `enterprise_key_login` + SQLite 本地缓存 + `can_access` 强制 + 单测。
- **阶段 3（服务端 + 官网 UI）**：实现官网账号/组织/团队/key/授权管理与网页管理界面。
- **阶段 4（联调）**：客户端 key 登录 ↔ 服务端校验 ↔ 功能门禁端到端验证。

---

## 9. 已确认决策汇总（全部锁定）

| # | 维度 | 决策 |
| --- | --- | --- |
| 1 | 持久化 | 服务端 SQLite；客户端授权缓存 SQLite（与量化数据湖 DuckDB 解耦） |
| 2 | 鉴权 | 新建服务端，账号密码登录（服务端）；本地客户端用 **key** 登录 |
| 3 | 组织粒度 | 用户在官网新建组织、加入人员；每人到平台申请 key；本地客户端用 key 登录 |
| 4 | 管理 UI | 官网网页内，不在客户端 |
| 5 | 本轮落点 | 客户端 + 服务端**一起交付** |
| 6 | 服务端归属 | **新建独立服务**（独立仓库/技术栈），与现有 workdsh 后台解耦 |
| 7 | 离线强制 | **本地优先**，无网按缓存授权门禁，联网后刷新校验 |
| 8 | key 形态 | **结构化 token**（带组织/用户前缀，如 `org.user.xxxx`） |
| 9 | 服务端技术栈 | **Rust + Axum + SQLite**（独立仓库） |
| 10 | 启动方式 | **契约优先**（先定稿 API/token/表结构契约，再并行实现两端） |

> 契约草案见第 10 节（已含 SQLite 表结构、结构化 token 形态、Axum 路由、客户端缓存 schema）。实现时再细化各 API 的请求/响应字段，不改变本方案的架构与契约。

---

## 10. 服务端技术栈与契约草案（Rust + Axum + SQLite）

### 10.1 技术栈与仓库

- **语言/框架**：Rust + **Axum**（HTTP 路由）+ Tokio（异步运行时）。
- **持久化**：**SQLite**，连接池用 `sqlx`（`SqlitePool`，async，便于迁移与并发）；或 `rusqlite`（同步，客户端 carrier 同款）。服务端推荐 `sqlx`。
- **密码哈希**：`argon2` 或 `bcrypt`；**key** 用 `sha256(orgId:userId:random)` 存储哈希，明文仅返回一次。
- **仓库**：新建独立仓库（建议 `quantdsh-enterprise-server`），与 `dsh-tauri` 解耦；CI 产出独立二进制/容器。
- **管理 UI**：官网网页（同仓 `frontend/` 或独立），仅调用下列 API；客户端不内置管理界面。

### 10.2 SQLite 表结构（DDL 草案）

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE orgs (
  id            TEXT PRIMARY KEY,            -- qo_<uuid>
  name          TEXT NOT NULL,
  owner_user_id TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);

CREATE TABLE users (
  id           TEXT PRIMARY KEY,             -- qu_<uuid>
  org_id       TEXT NOT NULL REFERENCES orgs(id),
  display_name TEXT NOT NULL,
  email        TEXT NOT NULL,
  password_hash TEXT NOT NULL,               -- argon2/bcrypt
  status       TEXT NOT NULL DEFAULT 'active', -- active|disabled
  role         TEXT NOT NULL DEFAULT 'member', -- owner|admin|member
  created_at   INTEGER NOT NULL,
  UNIQUE(org_id, email)
);

CREATE TABLE teams (
  id           TEXT PRIMARY KEY,             -- qt_<uuid>
  org_id       TEXT NOT NULL REFERENCES orgs(id),
  name         TEXT NOT NULL,
  description  TEXT NOT NULL DEFAULT '',
  created_by   TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  UNIQUE(org_id, name)
);

CREATE TABLE team_memberships (
  team_id   TEXT NOT NULL REFERENCES teams(id),
  user_id   TEXT NOT NULL REFERENCES users(id),
  role      TEXT NOT NULL DEFAULT 'member',  -- lead|member
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (team_id, user_id)
);

CREATE TABLE feature_grants (                -- "功能和团队挂上钩"的实体
  id            TEXT PRIMARY KEY,             -- qg_<uuid>
  org_id        TEXT NOT NULL REFERENCES orgs(id),
  team_id       TEXT NOT NULL REFERENCES teams(id),
  feature_key   TEXT NOT NULL,                -- data-hub|backtest|trading-desk|...
  feature_name  TEXT NOT NULL,
  access_level  TEXT NOT NULL,                -- view|use|manage
  granted_by    TEXT NOT NULL,
  granted_at    INTEGER NOT NULL
);
CREATE INDEX idx_grants_team ON feature_grants(team_id);

CREATE TABLE access_keys (                   -- 平台发放，客户端凭此登录
  id           TEXT PRIMARY KEY,             -- qk_<uuid>
  org_id       TEXT NOT NULL REFERENCES orgs(id),
  user_id      TEXT NOT NULL REFERENCES users(id),
  key_hash     TEXT NOT NULL,                -- sha256(plaintext)
  label        TEXT NOT NULL DEFAULT '',
  status       TEXT NOT NULL DEFAULT 'active', -- active|revoked
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
```

### 10.3 结构化 Token 形态

客户端登录用的 key = `org.user.xxxx`：

- `org` = 组织短码（URL-safe base64 或组织 slug），便于日志/路由识别。
- `user` = 用户短码。
- `xxxx` = 32 字节密码学随机十六进制；服务端只存 `sha256(plaintext)`。
- 校验：`POST /api/key/verify` 收到明文 → 计算 `sha256` → 比对 `access_keys.key_hash` → 命中且 `status=active` → 返回 `{ user, org, teams[], grants[] }`。

### 10.4 Axum 路由表（草案）

```
POST /api/auth/register           # 注册并建组织（首用户 = owner）
POST /api/auth/login              # 账号密码 → session/refresh（官网用）
POST /api/org/users              # 管理员增删改查用户              [admin]
POST /api/org/teams              # 创建/列出团队                  [admin|team-lead]
POST /api/org/teams/:id/members  # 增/删/改团队成员               [admin|team-lead]
POST /api/org/teams/:id/grants   # 给团队分配/撤销功能授权        [admin]
POST /api/keys/issue             # 当前用户申请 key（返回一次明文）
POST /api/key/verify             # 客户端 key 登录 → 授权快照
POST /api/keys/revoke            # 撤销 key
```

- 官网写操作需 `Authorization: Bearer <session>`（账号密码登录后拿）。
- `verify` 用 key 本身作 `Bearer <key>`。

### 10.5 客户端缓存（SQLite，Rust/rusqlite）

客户端 carrier 用 `rusqlite` 存一份精简快照，仅用于 `can_access` 离线判断，**不存密码/key 明文**：

```sql
CREATE TABLE entitlement_cache (
  user_id    TEXT PRIMARY KEY,
  org_id     TEXT,
  payload    TEXT NOT NULL,   -- JSON: {user,org,teams[],grants[]}
  fetched_at INTEGER NOT NULL
);
```

`can_access(feature_key)`：解析 `payload` → 收集 `user` 所属 `team_id` 集合 → 在 `grants` 中筛 `feature_key` 且 `team_id ∈ 集合` → 取最高 `access_level`。

### 10.6 实现顺序（契约优先）

1. 定稿本契约（本节）→ 两端并行开发。
2. **服务端**：建表迁移 + 路由 + 密码/key 哈希 + 官网管理 UI。
3. **客户端**：`enterprise_key_login` + SQLite 缓存 + `can_access` + 单测（mock 服务端）。
4. **联调**端到端：官网建组织/加人/建团队/分功能 → 客户端 key 登录 → 功能门禁生效。
