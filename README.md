# QuantDSH 桌面客户端载体

QuantDSH 桌面客户端由两层组成：**Rust + Tauri v2 的载体（carrier）** 负责身份、授权门禁、窗口、
运行时生命周期、进程树与安装器握手；**打包的 DSH runtime Profile** 提供全部业务界面
（数据中枢 / 策略工坊 / 交易台 / 投研智能体）。

安装包一次性带齐 **Node 24.21.0 + pnpm 11.28.5 + Python 3.12.14 + 完整 runtime Profile**，
用户机器**不需要预装 node / pnpm / python / git**。

```
┌────────────────────────────┐   spawn（detached / Job Object）  ┌──────────────────────────────┐
│ QuantDSH 主进程（carrier）   │ ────────────────────────────────> │ 打包的 Node + DSH Host       │
│ 协议 · 窗口 · 菜单 · 授权门禁 │                                  │ launcher → `dsh web --no-open`│
│ 进程树 · 单实例 · 更新校验    │ <── stdout: dsh web: http://… ─── │ （由 Profile/patch 层驱动）   │
└────────────┬───────────────┘                                  └──────────────┬───────────────┘
             │ 加载 http://127.0.0.1:<port>/?token=…                            │ loopback HTTP/WS
┌────────────▼───────────────┐                                                 │
│ 系统 WebView（WebView2/WK）│ ◄───────────────────────────────────────────────┘
└────────────────────────────┘
```

## 一、职责边界

| 层 | 位置 | 负责 | 不负责 |
| --- | --- | --- | --- |
| 官网 / 平台 | Rust + Axum + SQLite | 账号、组织、团队、功能授权、Key 发放、审计、共享资产 | 实盘凭据、策略源码、行情落库 |
| 载体（carrier） | 本仓库（Tauri v2 / Rust） | key 会话、`can_access` 门禁、授权快照缓存、窗口与菜单、进程树、单实例、安装器握手、运行时自愈、更新校验 | 业务 UI、数据分析、交易执行 |
| runtime Profile | 安装包内置 | 全部业务界面与工作区：数据中枢、策略工坊、交易台、投研智能体；DuckDB 本地数据湖 | 身份权威、授权签发 |

> **红线**：实盘凭据、交易所密钥、策略源码**绝不上传服务端**。服务端只持有身份与授权。

### 内部标识（已全量改为 `quantdsh`）

运行时标识已全部改为 `quantdsh` / `QuantDSH`：`quantdsh://` scheme、`quantdsh.localhost` host、
`quantdsh-runtime` 资源目录、`runtime_pack::PRODUCT = "QuantDSH"`、`%APPDATA%\QuantDSH` 数据根、
`QUANTDSH_*` 环境变量、`profiles\quantdsh` profile 名、`quantdsh-config.json` 部署配置。

改名**不考虑与旧版兼容，不写数据迁移**：旧数据目录、旧状态文件、旧环境变量、旧分区 cookie 一律不再读取，
相应的兼容分支已直接删除。依据：上游 `deepseek-harness` 全仓无 `quantdsh` 标识（装配产物
`primary-runtime` 也 0 命中），所以只需改本仓库（carrier），**不需要重建运行时**。

改名后 `cargo test` 40 项全通过，与改名前的基线一致。

### 仍是 `workdsh` 的地方（仅上游出处）

Profile 源码已 fork 进本仓库 `profile/`（`apps/web` + `packages` + `profiles` + `upstream.json`，
约 21 MB / 1300 文件；不含 `node_modules`、构建产物与官网站点）。npm 包名、cordis 插件 id、
服务键、插槽名已与运行时标识一起全部改为 `quantdsh`：

- 包名 `quantdsh-contracts` / `quantdsh-ui` / `quantdsh-bundle` / `quantdsh-plugin-*` / `quantdsh-provider-*`
- cordis 插件 id `quantdsh-identity-local`、`quantdsh-session-access`、`quantdsh-tool-access`、`quantdsh-workbenches` 等
- 服务键 `quantdshConnectors` / `quantdshExperts` / `quantdshLibrary` / `quantdshIdentity` / `quantdshAccess` 等
- 插槽名 `quantdsh.workbench.frame` —— 字节补丁的 `patchedSha256` 已按新替换内容重算（`originalSha256` 不变）

**唯一保留 `workdsh` 的是 profile 内指向上游真实仓库的外部链接**（`github.com/techflag/workdsh`、
`techflag.github.io`、star-history）。这些是出处标注，改掉就变成死链，因此原样保留。

## 二、模块职责

| 模块 | 职责 |
| --- | --- |
| `app.rs` | 全局状态机（`runtime`/`window`/`quitting`/`switching`/`quitRequested`/`returnRequested`/`installer`/`enterpriseWindows`/`verifyTimer`）、`openWindow`、`runtimeEnvironment`、`startRuntime`、`stopRuntime`、`leaveWorkspace`、`returnToEntry`、`endSwitch`、`activateEnterprise`、登录 IPC、菜单 |
| `paths.rs` | `QUANTDSH_*` 环境变量覆盖、`userData/dsh-home`、`quantdsh-runtime` 布局解析 |
| `connection.rs` | `DesktopConnection`、portal 校验、渲染进程分区标识 |
| `deployment_config.rs` | 管理员部署配置校验 |
| `enterprise_auth.rs` | key 登录 / `verify` / `cancelRequests` / `logout`，15s 超时、常量时间比对的 loopback authority（Bearer key、`/logout`、`/invalid`） |
| `enterprise_plugins.rs` | 企业插件探测与组合安装 |
| `local_runtime.rs` | Profile 物化、`cordis.patch.yml` 合成、企业 auth 补丁、`officialHostLauncher` |
| `official_command.rs` | 官方命令执行（超时 / 取消 / 进程登记） |
| `owned_process.rs` | 只停自己的进程树；Windows 用 kill-on-close **Job Object**（按句柄，免疫 PID 复用），失败回退到「代次校验 + `taskkill /T /F`」 |
| `runtime_port.rs` | Home 自有的 loopback 端口记录与 `webserver` patch（与 js-yaml 输出逐字节一致） |
| `runtime_diagnostics.rs` | 输出脱敏、失败详情、`runtime-failure.log` |
| `runtime_pack.rs` | 运行时定位、校验、离线包导入、修复页 |
| `runtime_compatibility.rs` | Profile 兼容性同步 |
| `plugin_archive_cache.rs` | 插件归档缓存 |
| `command_management.rs` | 「终端命令 dsh（可选）」的检查 / 安装 / 移除 |
| `pages.rs` | 载体页 HTML（入口 / 登录 / 加载）+ 等价 preload 的 init script；运行时修复页 |
| `browser_worker.rs` | `--quantdsh-browser-worker-port/profile` 离屏 worker（Windows 走 CDP `--remote-debugging-address=127.0.0.1 --remote-debugging-port`） |
| `runtime-cli/command-cli.mjs` | 已安装的 `dsh` 命令读取 `command-context.json`，解析 `--quantdsh-space=`，在选定空间里拉起官方 CLI |

### 渲染进程的 IPC 面

- 载体窗口（入口 / 登录 / 加载）：只有两个命令 `quantdsh_navigate`、`quantdsh_enterprise_login`，
  即唯一一条 preload 面；页面里的 `quantdsh://personal|enterprise|entry` 点击/表单提交被
  init script 拦截后走这两个命令。
- 运行时窗口（DSH 自己的页面）：**不注入任何 preload、不注册应用命令 ACL**，
  因此页面拿不到可用的 IPC —— 运行时页面与载体页严格隔离。

## 三、启动时序

1. `run()` 先解析 `--quantdsh-browser-worker-*`：命中则进入离屏 worker 分支并结束。
2. 主分支先注册单实例插件（`--dsh-installer-quit` 走有序退出）。
3. `start()`：读部署配置 → 读上次企业后台 → 安装菜单 → `showConnectionEntry()`。
4. 「个人使用」（或已存 key 的自动登录）：`showLoading` → `prepareProfile`（Profile 物化 + 依赖更新）→ `startRuntime`。
5. `startRuntime`：`prepareRuntimePort` → `officialHostLauncher` → spawn（Windows 挂 Job Object）
   → 解析 stdout 的 ready URL → `openWindow(url, connection)`。
6. 退出 / 切换：`before-quit` 的编舞（`quitting`/`quit_requested`/`switching`）→ `leaveWorkspace`
   → 关闭 authority → `stopOwnedProcess` → 清理 session 存储 → 销毁企业窗口。

## 四、构建与发布

前置：

1. Rust 工具链；Windows 上首次构建会自动下载 NSIS。
2. 本目录 `npm install` —— 提供 `@tauri-apps/cli`、`7zip-bin`（解包官方产物用的
   `7za.exe`）、`sharp`、`js-yaml`。
3. **Profile 源目录先构建出自有的 13 个包**。`pack-quantdsh-profile.mjs` 读的是 Profile 源下的
   `apps/web`：只有它的 `package.json` 钉着 `@deepseek-ai/dsh` 版本，也只有它装了 `pnpm`；
   它的 `PACKAGE_DIRECTORIES` 用 `../../packages/...`，正好落在 Profile 源的 `packages/`。
   仓库根不满足这些条件，传根目录会直接抛 `Usage:` 或版本断言。
   ```sh
   cd profile/apps/web            # 本仓库内的 profile fork（可用 --source / QUANTDSH_SOURCE 覆盖）
   corepack pnpm install --frozen-lockfile
   corepack pnpm build
   ```

首次发布还会下载锁定的官方 Desktop 产物（`upstream.json`：win-x64 为
289,385,456 B，SHA-512 校验），解出 Node/pnpm/Python/office-skills 约 257 MB，
缓存在 `build/official-release-cache/`，之后复用；`build/quantdsh-runtime` 约 900 MB。

```sh
npm run release:win        # Windows x64：全链路 + 打包 + 校验
npm run release:mac-arm    # macOS Apple Silicon
npm run release:mac-x64    # macOS Intel
```

`--source <dir>` 或 `QUANTDSH_SOURCE` 可覆盖 Profile 源位置（默认值见上一节）。

### 4.1 本地 debug 构建（只做冒烟测试）

```sh
npx tauri build --debug --no-bundle     # 只出 exe，不产安装包
```

产物是 `src-tauri/target/debug/QuantDSH.exe`。**不要给它设 `CARGO_TARGET_DIR`**：target 落在仓库内
（`.gitignore` 已忽略 `src-tauri/target/`），构建产物跟着仓库走，不占系统盘。磁盘预算约 3 GB，
其中 0.88 GB 是 Tauri 把 `build/quantdsh-runtime` 原样复制进 `target/debug/` 的那份资源（复制是必需的，
它让 debug 版能脱离仓库目录独立运行）。

`src-tauri/Cargo.toml` 的 `[profile.dev]` 按「只用于冒烟测试」调过：

| 设置 | 值 | 原因 |
|---|---|---|
| `debug` | `"line-tables-only"` | 默认 `2` 会给整棵 tauri/tokio/reqwest 依赖树各出一份完整 PDB，是 debug 目录膨胀的主因；行表保留栈回溯可读性 |
| `[profile.dev.package."*"].debug` | `false` | 依赖的调试信息对冒烟测试没有价值 |
| `incremental` | `false` | 省掉 `target/debug/incremental` 里每个 CGU 的副本；从零构建时它只花磁盘不省时间 |

实测：`target` 共 **2.56 GB**，其中 `deps` 1.31 GB、复制的 runtime 0.88 GB、`build` 0.31 GB，
主 crate 的 `QuantDSH.pdb` 只有 **36.5 MB**，exe 25.5 MB，`incremental` 为空。全量构建约 6 分钟。

启动后默认停在入口页，需要手动点「个人使用」。无人值守测试用 debug-only 开关
（`debug_assertions` 门控，发布版完全不读）：

```powershell
$env:QUANTDSH_DESKTOP_USER_DATA='D:\tmp\qdsmoke'    # 隔离数据目录，不碰真实配置
$env:QUANTDSH_DESKTOP_DEBUG_AUTOSTART='personal'    # 走与 quantdsh://personal 相同的路径
.\src-tauri\target\debug\QuantDSH.exe
```

首次进入要装 Profile 依赖，**约 60–120 秒才监听 `127.0.0.1:3080`**；这段时间 host 进程已在跑但端口没开，
容易被误判成启动失败。冒烟判据是：窗口标题 `QuantDSH`、host 为 carrier 的子进程、3080 返回 401（正常鉴权）。

> **窗口显示「请检查企业地址、网络或者本地运行状态」时先别怀疑网络。** 这个页面是 `page.loadURL()`
> 在 30 秒内没等到加载完成后的回滚（`window-2 never loaded`）。最常见的原因是上一次调试把进程
> 强杀了，WebView2 的缓存目录被弄脏，重页面（工作区首屏要载入几十个插件 client.js）在冷缓存上
> 加载超过 30 秒。清掉缓存目录再启动即可：
>
> ```powershell
> # 关掉所有 QuantDSH 与 host 进程后
> Remove-Item -Recurse "$env:LOCALAPPDATA\io.quantdsh.dsh.ssh\EBWebView"
> ```
>
> 排查顺序：先确认 `Get-NetTCPConnection -LocalPort 3080` 在监听、`curl http://127.0.0.1:3080/` 返回
> 401（未鉴权是正常的）、`http://127.0.0.1:3080/?token=…` 返回 200 的 HTML —— host 健康的话问题就在
> WebView2 侧，不在网络也不在构建。

`scripts/release.mjs` 的九步：

1. `prepare-deployment-config.mjs` — 生成 `build/deployment/quantdsh-config.json`
2. `pack-quantdsh-profile.mjs <apps/web> <candidate>` — 把自有插件打成不可变 tgz。
   候选目录用时间戳命名（`build/quantdsh-profile-release-<毫秒>/`），因为该脚本拒绝
   覆盖已存在的候选；固定路径会让同一份代码的第二次发版失败。
3. `prepare-quantdsh-runtime.mjs` — 合成并校验可移植 runtime Profile（并清掉
   `node_modules/**/*.pdb`，因为 `bundle.resources` 没有 filter 语义）
4. `prepare-quantdsh-primary-runtime.mjs` — 挂载锁定的 Node/pnpm/Python
5. `prepare-quantdsh-command.mjs` — 内置命令管理器与 `dsh` 命令
6. `generate-icons.mjs` — 从品牌图生成 Tauri 图标集
7. `make-runtime-pack.mjs` — 把同一份 runtime 打成**离线修复包**
   `build/packages/runtime-<ver>-<target>.tar.gz` + `.sha256`（详见第七节）
8. `npx tauri build --bundles <nsis|app>` — 编译并出安装包。每个平台只产一种包
   （`win.target: nsis/x64`、`mac.target: dir`）；产物名固定为
   `QuantDSH-<version>-x64-Setup.exe`（Tauri 默认是 `QuantDSH_<version>_x64-setup.exe`）
9. `verify-packaged.mjs` — 解包安装包，逐项校验运行时、Profile、导出面与 lockfile

也可以用 `--skip-build` 只跑 1–7 步（第 7 步的离线包本身就是要交付的产物）。

## 五、安装器行为（NSIS）

`src-tauri/installer-hooks.nsh` 负责让「升级安装」不撞上正在运行的实例：

1. 用 `FileOpen "$INSTDIR\QuantDSH.exe" a` 探测「该二进制是否正在运行」
   （运行中的进程会锁定自己的映像，因此不会误伤 `$INSTDIR` 下的 Node/Python）。
2. 通过单实例通道 `QuantDSH.exe --dsh-installer-quit` 请求有序退出（与用户退出同一套编舞）。
3. 最多等 30 秒；超时后弹窗确认，再 `taskkill /F /IM "QuantDSH.exe"` 精确关闭；
   仍失败则 `MB_RETRYCANCEL` 重试。

Tauri 自带的 `CheckIfAppIsRunning` 紧随其后执行，覆盖遗留情况。

## 六、验证

```sh
cd src-tauri && cargo test --lib     # 40 个单元测试：页面逐字节、URL 归一、patch 输出、运行时定位与导入、输出尾部裁剪、宿主启动脚本的 file URL 往返……
cd src-tauri && cargo check --all-targets
node scripts/verify-packaged.mjs --installer <安装包>   # 或 --root <解包目录>
```

`--installer` 先用 7-Zip 解包，解不开时（`7zip-bin` 附带的 `7za` 不带 NSIS 解析器，这是默认情形）
改为 `/S /D=<临时目录>` 静默安装、校验完再静默卸载 —— 这比解包更强：它真的跑了一遍安装脚本，
包括 `installer-hooks.nsh` 的退出握手。Windows 上这两步都会写
`HKCU\...\Uninstall` 与开始菜单快捷方式，校验结束会一并清掉。

安装器退出握手的回归方式：启动主实例 → 用 `--dsh-installer-quit` 启动第二个实例 →
断言第二实例退出码 0、主实例停止、运行标记被清理。隔离开关是
`QUANTDSH_DESKTOP_USER_DATA` 与 `QUANTDSH_DSH_HOME`。

## 七、运行时缺失与自愈

残缺的运行时不允许启动 Host（**绝不拿半棵目录树去启动**），但失败是可修复的
（`src-tauri/src/runtime_pack.rs`）。

定位顺序 —— **先验证，再采用**，第一个通过的胜出：

| 顺序 | 来源 | 说明 |
| -- | --- | --- |
| 1 | `QUANTDSH_RUNTIME_DIR` | 管理员显式指定；不完整时**直接失败**，不悄悄回落到别处 |
| 2 | `<resources>/quantdsh-runtime` | 安装包内置；完整时按内置布局启动 |
| 3 | `<userData>/runtime/current.json` → `versions/<sha256>` | 导入的离线包 |
| 4 | 都没有 | 打开修复页 `quantdsh://localhost/runtime`，**不启动** Host |

校验只做存在性 + 平台/架构 + 官方启动器需要的九个文件（`officialHostLauncher` 会解引用的
那几项），不哈希——每次启动都跑，289MB 不能重算。

修复页三个动作：**导入运行时离线包** / 打开下载页（配置了
`QUANTDSH_RUNTIME_DOWNLOAD_URL` 时才有）/ 退出。导入成功后立即重启进程
（`AppHandle::restart`），让新的 runtime 根目录在全新进程里重新解析，而不是在活的进程树
下面热替换运行时。

离线包 `runtime-<ver>-<target>.tar.gz` 由发布链第 7 步产出，内部结构：

```
manifest.json      # protocol / product / version / target / runtime + 逐文件 size & sha256
primary-runtime/…  # Node 24.21.0 · pnpm 11.28.5 · Python 3.12.14
profiles/quantdsh/…
```

导入（`import_archive`）严格按顺序进行：

1. `install.lock` 用 `create_new` 独占；**中断的安装故意留锁**，绝不猜属主是否还活着。
2. 解到同卷 `.staging-<uuid>`，逐条拒绝绝对路径与 `..`。
3. 用 `manifest.json` 逐文件校验 size + sha256，再用**真实启动器要求的那九项**做一次结构校验。
4. 摘要 = `sha256(manifest.json)`，`rename` 到 `versions/<摘要>`；已存在则复用（幂等）。
5. `current.json` 走临时文件 + `fsync` + `rename`，指针永远原子切换。
6. 清理其它版本目录（只留当前；回滚 = 重新导入旧包，是明确的用户动作，不自动猜）。

**为什么不复用系统自带的 node / python / pnpm 与 `dsh`**：Profile 的 `node_modules` 里有按
Node 24.21.0 ABI 编译的 `.node` 原生插件，pnpm 的 store 布局与 `lockfileVersion` 强绑定，
Python 侧 `numpy` 锁 2.3.5。而且版本号相同 ≠ 行为相同：nvm 的 symlink、fnm/volta 的 shim
（`dirname` 拿到的是转发器目录）、被重签名的 node、被继承的 `NODE_OPTIONS`、含空格或中文
的安装路径，都会在启动 30 秒后以「本机运行已停止」收场，且**无法归因**。还有一条与版本无关：
`enterpriseEnvironment()` 用 `dirname(node)` 前置 `PATH`，指向 `C:\Program Files\nodejs`
等于把整个系统目录暴露给 Host 及其全部子进程，企业隔离面当场破裂。

## 八、致命失败

没有额外的恢复面，只有三条明确出口：

| 失败 | 处理 |
| --- | --- |
| Host 子进程退出 | `failureDetail(code)` → 写 `logs/runtime-failure.log`（0600，覆盖写）→ 弹窗「本机运行已停止」→ 返回入口 |
| 进程清理未确认 | 同上，标题「本机运行失败，进程清理未确认」，**不**返回入口（进程树还在） |
| 启动 90 秒未就绪 | 弹窗「启动超时」/「官方 DSH 尚未就绪，已停止本机进程」→ 返回入口 |

`failureDetail` 的三件事：未就绪时的端口冲突提示、`runtimeFailureDetail` 正文、
末尾 `错误记录：<path>`；写盘失败时只是少这一行，弹窗内容其余不变。

托管子进程的 stdout/stderr 尾部保留 16,384 个 **UTF-16 单元**（与 `slice(-16_384)` 同义），
且裁剪必须落在字符边界上 —— 按字节切会在中文输出下 panic，而那个 panic 发生在读者任务里，
表现成「Host 突然不再输出」，无法归因。

## 九、关键实现约束

这些约束来自 WebView2 / wry / NSIS 的实际行为，改动前请先读「原因」列。

| 约束 | 做法 | 原因 |
| --- | --- | --- |
| 窗口显示时机 | 构建完成**立即** `show()`；`on_page_load` 的首个事件只负责销毁旧窗口 | WebView2 会挂起 `IsVisible(false)` 的 WebView：导航请求发得出去（连 303 重定向都能返回），但**文档永不提交**、`ContentLoading` 不触发，窗口永远停在 `about:blank` 并被看门狗回收 |
| Windows 路径 | `RuntimePaths::resolve` / `RuntimeLayout::resolve` 入口剥掉 `\\?\`；`official_launcher` / `official_host_launcher` 再兜一次；`path_to_file_url` 不转义 `:` | `resource_dir` 来自 `canonicalize()`，**必带** `\\?\`；原样拼 URL 会得到 `file://///%3F/D%3A/…`（非绝对路径），且 `:` 编码成 `%3A` 会让 Node 在解码**之前**判非绝对 → `ERR_INVALID_FILE_URL_PATH` |
| Profile 的 `node_modules` 布局 | `nodeLinker: hoisted` 扁平布局 | NSIS bundler 逐文件生成 `File "…"`，makensis 打不开超过 260 字符的源路径；isolated 的 `.pnpm` 前缀会多出约 93 字符，实测 **711 个文件超标（最长 331）**，hoisted 后最长 249。包版本由 `overrides` 统一钉死，解析结果不变 |
| 窗口加载失败的回滚 | 30s 看门狗触发 `roll_back_failed_window`：销毁新窗口、恢复 `window`/`entrySurface`、弹「无法打开工作区」 | wry 的 `on_page_load` 不上报 `did-fail-load`；看门狗是唯一能走到该分支的机制 |
| 隐藏浏览器 worker 的后台节流 | `additional_browser_args` 补 `--disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows` | Tauri 的 `background_throttling` 只落到 WKWebView，在 WebView2 上是空操作 |
| 缩放 | Windows 用 WebView2 `SetZoomFactor`；macOS/Linux 退回 `document.documentElement.style.zoom` | Tauri/wry 无跨平台 zoom API |
| 菜单文案 | 统一中文标签 | 产品为 zh-CN；**条目与快捷键集合保持完整**（编辑/视图/窗口三组） |
| macOS 呈现 | 打包后用 `patch-macos-info-plist.mjs` 写 `LSUIElement`，启动时 `set_activation_policy(Regular)` | worker 子进程不闪 Dock 图标，主应用仍显示 Dock 图标（Tauri 配置没有 extendInfo） |
| 渲染进程分区 | Windows/Linux 用 `data_directory`，macOS 用稳定的 `data_store_identifier` | WebView2/WKWebView 的等价隔离手段 |
| 打包校验 | `verify-packaged.mjs`（解包后校验） | Tauri bundler 无 afterPack 钩子，改为构建后校验 |

## 十、目录结构

```
dsh-tauri/
  scripts/                      # 发布链（1–9 步）与各项 verify 脚本
  build/deployment/             # 部署配置（管理员可定制）
  build/quantdsh-runtime/        # 打进安装包的 runtime（由第 3–5 步生成）
  build/packages/               # 第 7 步产出的离线修复包（tar.gz + sha256）
  assets/                       # 品牌图标（app-icon、tray 图标源）
  dist/index.html               # Tauri frontendDist 占位（应用自绘所有页面）
  src-tauri/
    src/                        # app.rs 等 17 个模块 + lib.rs / main.rs
    tauri.conf.json             # productName/version/identifier、resources、bundle、NSIS
    capabilities/default.json
    installer-hooks.nsh         # 安装器有序退出握手
    icons/                      # generate-icons.mjs 生成
    entitlements.mac.plist      # hardened runtime（Node/Python 需要 JIT）
```

## 十一、常见问题

- **首次发布为什么慢、为什么占空间**：要下载锁定的官方 Desktop 产物（289 MB）并按
  SHA-512 校验、从 npm 装出 862 个包的 Profile、gzip 出约 900 MB 的离线修复包，最后才
  编译 Rust 并压 NSIS。官方产物缓存在 `build/official-release-cache/`（含 `receipt.json`），
  第二次发布直接复用；`build/quantdsh-runtime` 约 900 MB。
- **`npx tauri build` 报缺少 `quantdsh-runtime`**：先跑 1–5 步（或 `npm run pack:runtime`）。
  构建脚本要求 `build/quantdsh-runtime` 与 `build/deployment/quantdsh-config.json` 存在。
- **启动后只看到「本机运行时不可用」**：内置 runtime 被删 / 被安全软件清理 / 解压不完整。
  点「导入运行时离线包」，选发布链产出的 `runtime-<ver>-<target>.tar.gz`；导入成功后自动重启。
  企业可以把它铺到任意目录并设 `QUANTDSH_RUNTIME_DIR`，或用
  `QUANTDSH_RUNTIME_DOWNLOAD_URL` 配一个内网下载页。
- **离线包导入报「另一个安装进程正在写入运行时」**：说明 `%APPDATA%\QuantDSH\runtime\install.lock`
  还在（上次安装被强杀）。确认没有安装在进行后删掉它，重试即可。
- **`QUANTDSH_DESKTOP_USER_DATA` / `QUANTDSH_DSH_HOME`**：分别为用户数据目录与 DSH Home 的
  覆盖点，便于测试与并排安装。
- **企业登录不可用**：入口页会禁用按钮，因为 `workdsh-provider-identity-enterprise`
  未装在该 Profile 里。
- **点「个人使用」报 `ERR_INVALID_FILE_URL_PATH: File URL path must be absolute`**：
  这是 Windows 上的路径前缀问题，已在源码层修掉（`simplify_verbatim` + `path_to_file_url`），
  两层根因见第九节。
- **点了「个人使用」窗口不出现（或一直空白、30 秒后被关掉）**：不要用
  `visible(false)` 建窗口再等加载完显示（原因见第九节）。本实现在
  `WebviewWindowBuilder::build()` 之后立刻 `show()`（背景色已是 `#111113`，不会白闪），
  「加载完成」只用来销毁上一个窗口。验证时看 stderr 的
  `[quantdsh] window <label> ready at <url>`——它只在首帧打印一次。
