# QuantDSH（Tauri 版）

把 workdsh 的 Electron 桌面载体（`workdsh/apps/desktop`，产品名 QuantDSH 3.0.0-alpha.3）  
**逐个行为地复刻**为 Tauri v2 + Rust 应用：同样的入口页/企业登录/加载页、同样的官方 Host  
启动协议（`dsh web: http://127.0.0.1:<port>/?token=…`）、同样的企业认证桥、同样的进程树清理、  
同样的单实例与安装器退出握手、同样的 NSIS 安装体验。

安装包一次性带齐 **Node 24.21.0 + pnpm 11.28.5 + Python 3.12.14 + 完整 runtime Profile**，  
用户机器**不需要预装 node / pnpm / python / git**。

```
┌────────────────────────────┐   spawn（detached / Job Object）  ┌──────────────────────────────┐
│ Tauri Rust 主进程（carrier）│ ────────────────────────────────> │ 打包的 Node + 官方 DSH Host  │
│ 协议 workdsh:// · 窗口 · 菜单│                                  │ launcher → `dsh web --no-open`│
│ 企业认证 · 进程树 · 单实例   │ <── stdout: dsh web: http://… ─── │ （由 Profile/patch 层驱动）   │
└────────────┬───────────────┘                                  └──────────────┬───────────────┘
             │ 加载 http://127.0.0.1:<port>/?token=…                            │ loopback HTTP/WS
┌────────────▼───────────────┐                                                 │
│ 系统 WebView（WebView2/WK）│ ◄───────────────────────────────────────────────┘
└────────────────────────────┘
```

## 一、与 Electron 源文件的对应关系

`src-tauri/src/lib.rs` 顶部有同一张表；Rust 模块与 TypeScript 源码一一对应，便于逐行核对：

| Electron / workdsh                                     | Tauri / Rust                  | 复刻内容                                                                                                                                                                                                                                                                        |
| ------------------------------------------------------ | ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `workdsh-main.ts`                                      | `app.rs`                      | 全局状态机（`runtime`/`window`/`quitting`/`switching`/`quitRequested`/`returnRequested`/`installer`/`enterpriseWindows`/`verifyTimer`）、`openWindow`、`runtimeEnvironment`、`startRuntime`、`stopRuntime`、`leaveWorkspace`、`returnToEntry`、`endSwitch`、`activateEnterprise`、登录 IPC、菜单 |
| 路径函数（`bundledProfileDirectory` 等）                      | `paths.rs`                    | `WORKDSH_*` 环境变量覆盖、`userData/dsh-home`、`workdsh-runtime` 布局                                                                                                                                                                                                                 |
| `connection-mode.ts`                                   | `connection.rs`               | `DesktopConnection`、portal 校验、Electron partition 名                                                                                                                                                                                                                          |
| `deployment-config.ts`                                 | `deployment_config.rs`        | 管理员部署配置校验                                                                                                                                                                                                                                                                   |
| `enterprise-auth.ts`                                   | `enterprise_auth.rs`          | 登录/`verify`/`cancelRequests`/`logout`，以及 15s 超时、常量时间比对的 loopback authority（Bearer key、`/logout`、`/invalid`）                                                                                                                                                                 |
| `enterprise-plugins.ts`                                | `enterprise_plugins.rs`       | 企业插件探测与组合安装                                                                                                                                                                                                                                                                 |
| `local-runtime.ts`                                     | `local_runtime.rs`            | Profile 物化、`cordis.patch.yml` 合成、企业 auth 补丁、`officialHostLauncher`                                                                                                                                                                                                          |
| `official-command.ts`                                  | `official_command.rs`         | 官方命令执行（超时/取消/进程登记）                                                                                                                                                                                                                                                          |
| `owned-process.ts`                                     | `owned_process.rs`            | 只停自己的进程树；Windows 用 kill-on-close **Job Object**（按句柄，免疫 PID 复用），失败回退到「代次校验 + `taskkill /T /F`」                                                                                                                                                                               |
| `runtime-port.ts`                                      | `runtime_port.rs`             | Home 自有的 loopback 端口记录与 `webserver` patch（与 js-yaml 输出逐字节一致）                                                                                                                                                                                                                |
| `runtime-diagnostics.ts`                               | `runtime_diagnostics.rs`      | 输出脱敏、失败详情、`runtime-failure.log`                                                                                                                                                                                                                                             |
| `command-management.ts`                                | `command_management.rs`       | 「终端命令 dsh（可选）」的检查/安装/移除                                                                                                                                                                                                                                                     |
| `connection-entry.ts` + `connection-preload.ts`        | `pages.rs`                    | 载体页 HTML（逐字节对齐）+ 等价 preload 的 init script；另有一张 Tauri 独有面：运行时修复页                                                                                                                                                                                                             |
| `workdsh-main.ts` 的 `startBrowserWorker` 分支            | `browser_worker.rs`           | `--workdsh-browser-worker-port/profile` 离屏 worker（Windows 走 CDP `--remote-debugging-address=127.0.0.1 --remote-debugging-port`）                                                                                                                                             |
| `command-cli.ts`                                       | `runtime-cli/command-cli.mjs` | 已安装的 `dsh` 命令读取 `command-context.json`，解析 `--workdsh-space=`，在选定空间里拉起官方 CLI                                                                                                                                                                                                 |
| `plugin-archive-cache.ts` / `runtime-compatibility.ts` | 同名模块                          | 归档缓存与兼容性同步                                                                                                                                                                                                                                                                  |

### 渲染进程的 IPC 面（这一点与 Electron 完全一致）

- 载体窗口（入口/登录/加载）：只有两个命令 `workdsh_navigate`、`workdsh_enterprise_login`，  
  等价于 Electron 的唯一 preload；页面里的 `workdsh://personal|enterprise|entry`  
  点击/表单提交被 init script 拦截后走这两个命令（对应 Electron 的 `will-navigate` 拦截）。
- 运行时窗口（DSH 自己的页面）：**不注入任何 preload、不注册应用命令 ACL**，  
  因此页面拿不到可用的 IPC —— 与 Electron 运行时窗口无 preload 的隔离强度相同。

## 二、启动时序（与 `workdsh-main.ts` 对齐）

1. `run()` 先解析 `--workdsh-browser-worker-*`：命中则进入离屏 worker 分支并结束。
2. 主分支先注册单实例插件（`--dsh-installer-quit` 走有序退出）。
3. `start()`：读部署配置 → 读上次企业后台 → 安装菜单 → `showConnectionEntry()`。
4. 「个人使用」：`showLoading` → `prepareProfile`（Profile 物化 + 依赖更新）→ `startRuntime`。
5. `startRuntime`：`prepareRuntimePort` → `officialHostLauncher` → spawn（Windows 挂 Job Object）  
   → 解析 stdout 的 ready URL → `openWindow(url, connection)`。
6. 退出/切换：`before-quit` 的编舞（`quitting`/`quit_requested`/`switching`）→ `leaveWorkspace`  
   → 关闭 authority → `stopOwnedProcess` → 清理 session 存储 → 销毁企业窗口。

## 三、构建与发布

前置：

1. Rust 工具链；Windows 上首次构建会自动下载 NSIS。
2. 本目录 `npm install` —— 提供 `@tauri-apps/cli`、`7zip-bin`（解包官方产物用的  
   `7za.exe`）、`sharp`、`js-yaml`。
3. **workdsh 仓库先构建出自有的 13 个包**。`pack-workdsh-profile.mjs` 读的是  
   `apps/web`：只有它的 `package.json` 钉着 `@deepseek-ai/dsh` 版本，也只有它装了  
   `pnpm`；它的 `PACKAGE_DIRECTORIES` 用 `../../packages/...`，正好落在仓库的  
   `packages/`。仓库根不满足这些条件，传根目录会直接抛 `Usage:` 或版本断言。
   ```sh
   cd ../workdsh
   corepack yarn install --immutable
   corepack pnpm --dir apps/web install --frozen-lockfile
   corepack pnpm --dir apps/web build
   ```

首次发布还会下载锁定的官方 Desktop 产物（`upstream.json`：win-x64 为  
289,385,456 B，SHA-512 校验），解出 Node/pnpm/Python/office-skills 约 257 MB，  
缓存在 `build/official-release-cache/`，之后复用；`build/workdsh-runtime` 约 900 MB。

```sh
npm run release:win        # Windows x64：全链路 + 打包 + 校验
npm run release:mac-arm    # macOS Apple Silicon
npm run release:mac-x64    # macOS Intel
```

`--source <dir>` 或 `WORKDSH_SOURCE` 可指向别处的 `apps/web`（默认 `../workdsh/apps/web`）。

`scripts/release.mjs` 的九步等价于 Electron 的 release 链：

1. `prepare-deployment-config.mjs` — 生成 `build/deployment/workdsh-config.json`
2. `pack-workdsh-profile.mjs <apps/web> <candidate>` — 把自有插件打成不可变 tgz。  
   候选目录用时间戳命名（`build/workdsh-profile-release-<毫秒>/`），因为该脚本拒绝  
   覆盖已存在的候选；固定路径会让同一份代码的第二次发版失败。
3. `prepare-workdsh-runtime.mjs` — 合成并校验可移植 runtime Profile（并按 electron-builder  
   `extraResources.filter` 的排除项清掉 `node_modules/**/*.pdb`，因为 Tauri 的  
   `bundle.resources` 没有 filter 语义）
4. `prepare-workdsh-primary-runtime.mjs` — 挂载锁定的 Node/pnpm/Python
5. `prepare-workdsh-command.mjs` — 内置命令管理器与 `dsh` 命令
6. `generate-icons.mjs` — 从品牌图生成 Tauri 图标集
7. `make-runtime-pack.mjs` — 把同一份 runtime 打成**离线修复包**  
   `build/packages/runtime-<ver>-<target>.tar.gz` + `.sha256`（详见第六节）
8. `npx tauri build --bundles <nsis|app>` — 编译并出安装包。每个平台只产一种包，与  
   electron-builder 的 `win.target: nsis/x64`、`mac.target: dir` 一致；产物名也改回  
   `QuantDSH-<version>-x64-Setup.exe`（Tauri 默认是 `QuantDSH_<version>_x64-setup.exe`）
9. `verify-packaged.mjs` — 解包安装包，逐项校验运行时、Profile、导出面与 lockfile

也可以用 `--skip-build` 只跑 1–7 步（第 7 步的离线包本身就是要交付的产物）。

## 四、安装器行为（NSIS）

`src-tauri/installer-hooks.nsh` 是 `workdsh/apps/desktop/build/installer.nsh` 的 Tauri 移植：

1. 用 `FileOpen "$INSTDIR\QuantDSH.exe" a` 探测「该二进制是否正在运行」  
   （运行中的进程会锁定自己的映像，因此不会误伤 `$INSTDIR` 下的 Node/Python）。
2. 通过单实例通道 `QuantDSH.exe --dsh-installer-quit` 请求有序退出（与用户退出同一套编舞）。
3. 最多等 30 秒；超时后弹窗确认，再 `taskkill /F /IM "QuantDSH.exe"` 精确关闭；  
   仍失败则 `MB_RETRYCANCEL` 重试。

Tauri 自带的 `CheckIfAppIsRunning` 紧随其后执行，覆盖遗留情况。

## 五、验证

```sh
cd src-tauri && cargo test --lib     # 40 个单元测试：页面逐字节、URL 归一、patch 输出、运行时定位与导入、输出尾部裁剪、宿主启动脚本的 file URL 往返……
cd src-tauri && cargo check --all-targets
node scripts/verify-packaged.mjs --installer <安装包>   # 或 --root <解包目录>
```

`--installer` 先用 7-Zip 解包，解不开时（`7zip-bin` 附带的 `7za` 不带 NSIS 解析器，这是默认情形）  
改为 `/S /D=<临时目录>` 静默安装、校验完再静默卸载 —— 这比解包更强：它真的跑了一遍安装脚本，  
包括 `installer-hooks.nsh` 的退出握手。Windows 上这两步都会写  
`HKCU\...\Uninstall` 与开始菜单快捷方式，校验结束会一并清掉。

安装器退出握手沿用 workdsh 的 `#469` 回归思路  
（`workdsh/apps/desktop/scripts/probe-windows-installer-quit.ps1`）：启动主实例 → 用  
`--dsh-installer-quit` 启动第二个实例 → 断言第二实例退出码 0、主实例停止、运行标记被清理。  
本仓库的等价隔离开关是 `WORKDSH_DESKTOP_USER_DATA` 与 `WORKDSH_DSH_HOME`（Electron 用的是  
`--user-data-dir`）。

## 六、运行时缺失与自愈（Tauri 独有）

Electron 版对残缺运行时是硬失败：`startRuntime` 里  
`if (!existsSync(executable)) throw new Error('Bundled Node.js runtime is missing: …')`，  
用户看到一句英文弹窗后窗口就废了。本实现保留「**绝不拿半棵目录树去启动 Host**」这条契约，  
但把失败变成可修复的（`src-tauri/src/runtime_pack.rs`）。

定位顺序 —— **先验证，再采用**，第一个通过的胜出：

| 顺序 | 来源                                                      | 说明                                               |
| -- | ------------------------------------------------------- | ------------------------------------------------ |
| 1  | `WORKDSH_RUNTIME_DIR`                                   | 管理员显式指定；不完整时**直接失败**，不悄悄回落到别处                    |
| 2  | `<resources>/workdsh-runtime`                           | 安装包内置；完整时与 Electron 完全同一路径、同一行为                  |
| 3  | `<userData>/runtime/current.json` → `versions/<sha256>` | 导入的离线包                                           |
| 4  | 都没有                                                     | 打开修复页 `workdsh://localhost/runtime`，**不启动** Host |

校验只做存在性 + 平台/架构 + 官方启动器需要的九个文件（`officialHostLauncher` 会解引用的  
那几项），不哈希——每次启动都跑，289MB 不能重算。

修复页三个动作：**导入运行时离线包** / 打开下载页（配置了  
`WORKDSH_RUNTIME_DOWNLOAD_URL` 时才有）/ 退出。导入成功后立即重启进程  
（`AppHandle::restart`），让新的 runtime 根目录在全新进程里重新解析，而不是在活的进程树  
下面热替换运行时。

离线包 `runtime-<ver>-<target>.tar.gz` 由发布链第 7 步产出，内部结构：

```
manifest.json      # protocol / product / version / target / runtime + 逐文件 size & sha256
primary-runtime/…  # Node 24.21.0 · pnpm 11.28.5 · Python 3.12.14
profiles/workdsh/…
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
（`dirname` 拿到的是转发器目录）、企业重签名的 node、被继承的 `NODE_OPTIONS`、含空格或中文  
的安装路径，都会在启动 30 秒后以「本机运行已停止」收场，且**无法归因**。还有一条与版本无关：  
`enterpriseEnvironment()` 用 `dirname(node)` 前置 `PATH`，指向 `C:\Program Files\nodejs`  
等于把整个系统目录暴露给 Host 及其全部子进程，企业隔离面当场破裂。

## 七、致命失败

与 `workdsh-main.ts` 逐条对齐，没有额外的恢复面：

| 失败         | 处理                                                                                |
| ---------- | --------------------------------------------------------------------------------- |
| Host 子进程退出 | `failureDetail(code)` → 写 `logs/runtime-failure.log`（0600，覆盖写）→ 弹窗「本机运行已停止」→ 返回入口 |
| 进程清理未确认    | 同上，标题「本机运行失败，进程清理未确认」，**不**返回入口（进程树还在）                                            |
| 启动 90 秒未就绪 | 弹窗「启动超时」/「官方 DSH 尚未就绪，已停止本机进程」→ 返回入口                                              |

`failureDetail` 与 Electron 一致的三件事：未就绪时的端口冲突提示、`runtimeFailureDetail` 正文、  
末尾 `错误记录：<path>`；写盘失败时只是少这一行，弹窗内容其余不变。

托管子进程的 stdout/stderr 尾部保留 16,384 个 **UTF-16 单元**（与 `slice(-16_384)` 同义），  
且裁剪必须落在字符边界上 —— 按字节切会在中文输出下 panic，而那个 panic 发生在读者任务里，  
表现成「Host 突然不再输出」，无法归因。

> 官方新版 `deepseek-harness` 桌面端另有 `crash-report.ts` / `fatal-recovery.ts`（多份  
> `crash-*.log`、四个按钮的恢复对话框、禁用第三方插件）。**workdsh 没有这两块**，所以这里  
> 也不引入：复刻目标是 workdsh，与官方一致的只有打包架构（第三节的资源布局与第六节的自愈）。

## 八、与 Electron 版的已知差异（有意为之）

| 项目                          | Electron                                                                                | 本实现                                                                                                                                            | 原因                                                                                                                                                                                                                             |
| --------------------------- | --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 视图菜单的「强制重新加载」               | `reloadIgnoringCache()`                                                                 | 与「重新加载」相同的 `reload()`                                                                                                                          | WebView2 不暴露绕过缓存的重载；本应用的页面来自本机 loopback，无缓存语义差异                                                                                                                                                                                |
| 缩放                          | `webContents` zoom                                                                      | Windows 用 WebView2 `SetZoomFactor`；macOS/Linux 退回 `document.documentElement.style.zoom`                                                        | Tauri/wry 无跨平台 zoom API                                                                                                                                                                                                        |
| 菜单文案                        | role 菜单为 Electron 内置英文标签                                                                | 统一中文标签                                                                                                                                         | 产品为 zh-CN；**条目与快捷键集合与 `editMenu`/`viewMenu`/`windowMenu` 一致**                                                                                                                                                                  |
| macOS 呈现                    | `extendInfo.LSUIElement: true`（bundle 以 agent 启动）+ 主进程 `setActivationPolicy('regular')` | 打包后用 `patch-macos-info-plist.mjs` 写 `LSUIElement`，启动时 `set_activation_policy(Regular)`                                                         | 等价：worker 子进程不会闪 Dock 图标，主应用仍显示 Dock 图标（Tauri 配置没有 extendInfo）                                                                                                                                                                 |
| 渲染进程分区                      | `partition: persist:…`                                                                  | Windows/Linux 用 `data_directory`，macOS 用稳定的 `data_store_identifier`                                                                            | WebView2/WKWebView 的等价隔离手段                                                                                                                                                                                                     |
| 窗口加载失败的回滚                   | `page.loadURL(url).catch()` 立刻触发：销毁新窗口、恢复 `window`/`entrySurface`、弹「无法打开工作区」            | 同一个回滚分支，改由 30s 看门狗触发（`roll_back_failed_window`）                                                                                                | wry 的 `on_page_load` 不上报 `did-fail-load`；看门狗是唯一能走到该分支的等价机制                                                                                                                                                                     |
| 窗口显示时机                      | `show: false`，`ready-to-show`（首帧）再显示                                                    | 构建完成**立即** `show()`；`on_page_load` 的首个事件负责销毁旧窗口                                                                                                | WebView2 会挂起 `IsVisible(false)` 的 WebView：导航请求发得出去（`on_navigation` 会跑，连 303 重定向都能返回），但**文档永不提交**、`ContentLoading` 不触发，窗口永远停在 `about:blank` 并被看门狗回收。所以「隐藏构建 + 等加载完再显示」在 WebView2 上必然死锁（详见第十节）                                   |
| 隐藏浏览器 worker 的后台节流          | `webPreferences.backgroundThrottling: false`                                            | `additional_browser_args` 里补 `--disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows` | Tauri 的 `background_throttling` 只落到 WKWebView，在 WebView2 上是空操作                                                                                                                                                                 |
| 路径的 verbatim 前缀             | 无（Electron 自己拼 `file://`）                                                               | `RuntimePaths::resolve` / `RuntimeLayout::resolve` 入口剥掉 `\\?\`；`official_launcher` / `official_host_launcher` 再兜一次（它们负责把路径写进 JS 字面量）；`path_to_file_url` 不再转义 `:`                                                   | Tauri 的 `resource_dir` 在 Windows 上来自 `canonicalize()`，**必带** `\\?\`。原样拼 URL 会得到 `file://///%3F/D%3A/…`（非绝对路径）；就算把 `%3F` 修好，`%3A` 也会让 Node 在解码**之前**判定路径不绝对 → `ERR_INVALID_FILE_URL_PATH`（详见第十节）                                |
| 打包校验                        | `verify-workdsh-carrier.ts`（afterPack）                                                  | `verify-packaged.mjs`（解包后校验）                                                                                                                   | Tauri bundler 无 afterPack 钩子，改为构建后校验                                                                                                                                                                                           |
| Profile 的 `node_modules` 布局 | pnpm 默认 isolated：`.pnpm/<name>_<hash>/node_modules/…`                                   | `nodeLinker: hoisted` 扁平布局                                                                                                                     | electron-builder 把 `extraResources` 交给 7z，能处理超长路径；Tauri 的 NSIS bundler 逐文件生成 `File "…"`，而 makensis 打不开超过 260 字符的源路径。isolated 的 `.pnpm` 前缀会多出约 93 字符，实测 **711 个文件超标（最长 331）**；hoisted 后最长 249，全部通过。包版本由 `overrides` 统一钉死，解析结果不变 |
| 运行时缺失                       | `throw new Error('Bundled Node.js runtime is missing')` + 错误弹窗，窗口作废                     | 修复页（导入离线包 / 打开下载页 / 退出），导入后自动重启                                                                                                                | 契约不变（半棵目录树绝不启动 Host），但把不可恢复的失败变成可修复；见第六节                                                                                                                                                                                       |


## 九、目录结构

```
dsh-tauri/
  scripts/                  # 发布链（1–9 步）与各项 verify 脚本
  build/deployment/         # 部署配置（管理员可定制）
  build/workdsh-runtime/    # 打进安装包的 runtime（由第 3–5 步生成）
  build/packages/           # 第 7 步产出的离线修复包（tar.gz + sha256）
  assets/                   # 品牌图标（app-icon、tray 图标源）
  dist/index.html           # Tauri frontendDist 占位（应用自绘所有页面）
  src-tauri/
    src/                    # app.rs 等 17 个模块 + lib.rs / main.rs
    tauri.conf.json         # productName/version/identifier、resources、bundle、NSIS
    capabilities/default.json
    installer-hooks.nsh     # 安装器有序退出握手
    icons/                  # generate-icons.mjs 生成
    entitlements.mac.plist  # hardened runtime（Node/Python 需要 JIT）
```

## 十、常见问题

- **首次发布为什么慢、为什么占空间**：要下载锁定的官方 Desktop 产物（289 MB）并按
  SHA-512 校验、从 npm 装出 862 个包的 Profile、gzip 出约 900 MB 的离线修复包，最后才
  编译 Rust 并压 NSIS。官方产物缓存在 `build/official-release-cache/`（含 `receipt.json`），
  第二次发布直接复用；`build/workdsh-runtime` 约 900 MB。
- **`npx tauri build` 报缺少 `workdsh-runtime`**：先跑 1–5 步（或 `npm run pack:runtime`）。
  构建脚本要求 `build/workdsh-runtime` 与 `build/deployment/workdsh-config.json` 存在。
- **启动后只看到「本机运行时不可用」**：内置 runtime 被删/被安全软件清理/解压不完整。
  点「导入运行时离线包」，选发布链产出的 `runtime-<ver>-<target>.tar.gz`；导入成功后自动重启。
  企业可以把它铺到任意目录并设 `WORKDSH_RUNTIME_DIR`，或用
  `WORKDSH_RUNTIME_DOWNLOAD_URL` 配一个内网下载页。
- **离线包导入报「另一个安装进程正在写入运行时」**：说明 `%APPDATA%\WorkDSH\runtime\install.lock`
  还在（上次安装被强杀）。确认没有安装在进行后删掉它，重试即可。
- **`WORKDSH_DESKTOP_USER_DATA` / `WORKDSH_DSH_HOME`**：分别为 Electron
  `app.getPath('userData')` 与 DSH Home 的覆盖点，便于测试与并排安装。
- **企业登录不可用**：入口页会禁用按钮，因为 `workdsh-provider-identity-enterprise`
  未装在该 Profile 里（与 Electron 的 `enterprisePluginAvailable()` 判定完全一致）。
- **点「个人使用」报 `ERR_INVALID_FILE_URL_PATH: File URL path must be absolute`**：
  这是 Windows 上的路径前缀问题，已在源码层修掉（`simplify_verbatim` + `path_to_file_url`）。
  根因有两层：Tauri 的 `resource_dir` 由 `canonicalize()` 得出，必定是 `\\?\D:\…` 形式，
  而 `official_host_launcher` 会把 profile 的 `profile-boot.js`、`dsh-app-boot/lib/index.js`
  写成 `import … from "file://…"`。①`\\?\` 被逐字符转义成 `%3F`，URL 变成
  `file://///%3F/D%3A/…`，不再以 `/` 开头；②即使修好①，盘符冒号若被编码成 `%3A`，Node 的
  `getPathFromURLWin32` 会在**解码之前**检查 `pathname[1] === ':'`，依旧判为非绝对路径。
  所以 `:` 必须保持字面量。
- **点了「个人使用」窗口不出现（或一直空白、30 秒后被关掉）**：不要用
  `visible(false)` 建窗口再等加载完显示。WebView2 会把 `IsVisible(false)` 的 WebView 挂起：
  网络请求照发（服务端能看到 303/200），但文档不提交，`ContentLoading` 与
  `NavigationCompleted` 都不来。本实现因此在 `WebviewWindowBuilder::build()` 之后立刻
  `show()`（背景色已是 `#111113`，不会白闪），「加载完成」只用来销毁上一个窗口。
  `scripts/` 里没有测试钩子，验证时请看 stderr 的
  `[workdsh] window <label> ready at <url>`——它只在首帧打印一次。
