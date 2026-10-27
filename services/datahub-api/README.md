# datahub-api

QuantDSH 的行情数据服务。**替换**了原来的 Rust `quantdsh-datahub`（`src-tauri/datahub`，已停用），
对外 HTTP 契约保持不变：`/health` 免鉴权，`/api/*` 需要 `Authorization: Bearer <token>`。

设计文档：仓库根目录 `QUANTDSH_DATA_INGEST_DESIGN.md`（v2.0）。

## V1 范围

只做 **K 线（成交价）**，现货 `spot` + 永续 `swap`，数据通道只走 REST。
目标是先把「配置数据源 → 拉取 → 规范化 → 幂等落库 → 目录 → 覆盖率 → 体检」整条链路压通。

## 数据源在数据库里

`sources` 表（`meta.sqlite`）是唯一事实来源，代码里没有硬编码的交易所清单：

| id | provider | venue | 说明 |
|---|---|---|---|
| `binance` | `ccxt` | binance | 主域不可达时自动切 `data-api.binance.vision` |
| `okx` | `ccxt` | okx | |
| `bybit` | `ccxt` | bybit | |
| `official` | `official` | — | 我们自己的聚合 API，默认禁用 |
| `replay` | `replay` | replay | 离线确定性造数，用于回归与演示 |

关键区分：**venue 是「数据属于哪个交易所」，source 是「从哪条通道取」**。
`instrument_id` 只含 venue，所以同一份行情走直连或走官方 API 落的是同一条主键，
不会产生两份数据。换通道只改配置。

新增一家交易所：`POST /api/sources` 加一行 `provider=ccxt, venue=<ccxt id>`，无需改代码。
接入官方 API：编辑 `official` 那行的 `base_urls` 并 `enabled=1`，协议见设计文档 §4.5.3。

## 本地开发

```powershell
# 依赖（Python 3.12+；打包时用 scripts/prepare-python-runtime.mjs 自带的解释器）
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt

# 启动（token 会写进 <data-dir>/datahub.json）
.\.venv\Scripts\python.exe -m app --data-dir "$env:TEMP\quantdsh-dev"

# 另一个窗口
curl "http://127.0.0.1:17821/health"
```

`-m app` 的 `--data-dir` 必须指向存放 `market.duckdb` / `meta.sqlite` 的目录；
默认是 `%LOCALAPPDATA%\QuantDSH\data`，与桌面端一致。

## 测试

```powershell
# 端到端（离线，用 replay 源走真实写库+体检路径）
.\.venv\Scripts\python.exe scripts\smoke_test.py

# 真实网络（需要在能直连交易所的网络下跑）
.\.venv\Scripts\python.exe scripts\live_check.py binance BTC/USDT 1m
```

## 两个必须知道的实现约束

1. **DuckDB 版本锁死 `1.5.6`**（`requirements.txt`）。Rust 端 pin 的 engine 就是 1.5.6，
   2.x 的存储格式打不开既有的 `market.duckdb`。
2. **写 bar 只能用「临时 CSV + COPY」**，不能用 `executemany`。当前 DuckDB 在参数化插入
   累计约 1 万行时会直接 abort 进程（没有 traceback）。COPY 实测 51 万行 / 5.8 秒。

## 打包

安装包自带 Python 运行时，用户机器不需要装 Python：

```powershell
node scripts\prepare-python-runtime.mjs   # → build/python-runtime/python（206 MB）
```

`src-tauri/tauri.conf.json` 把它映射成 `runtime/`，把 `app/` 映射成 `datahub-api/app/`；
carrier 用 `runtime/python/python.exe -s -B -m app` 启动，日志写到 `<data_dir>/datahub.log`。

脚本是幂等的：解释器能 import 全部 wheel 时只做裁剪（删 `*.pdb`、`include/`、`libs/`，
291 MB → 206 MB），不会重新下载。网络到不了 GitHub 时用 `PYTHON_RUNTIME_URL` 指一个本地包。

## 验收自带运行时

下面两条命令证明「用户机器没有 Python 也能跑」，都不依赖系统解释器：

```powershell
# 1. 用自带解释器真实起一次服务：等 datahub.json → /health → 401 → 种子源 → schema
build\python-runtime\python\python.exe -s -B scripts\verify_bundled_runtime.py

# 2. 只用打包树里的 wheel（-s 忽略用户 site-packages）
build\python-runtime\python\python.exe -s -B -c "import fastapi, uvicorn, ccxt, duckdb"
```

实测冷启动 2.7 s（导入 fastapi + ccxt + duckdb）。`scripts/verify-packaged.mjs` 在发版时会
对**安装后的解释器**再跑一次同样的 import 检查，所以漏打包 wheel 不会静默通过。
