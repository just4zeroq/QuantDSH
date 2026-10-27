//! QuantDSH — the desktop carrier for the bundled DeepSeek Harness runtime.
//!
//! Each module owns one concern of the carrier process:
//!
//! | Module                    | Responsibility |
//! |---------------------------|----------------|
//! | [`app`]                   | 全局状态机、窗口与运行时生命周期 |
//! | [`paths`]                 | 运行时目录与 `QUANTDSH_*` 环境覆盖解析 |
//! | [`connection`]            | 渲染进程连接与分区标识 |
//! | [`deployment_config`]     | 管理员部署配置校验 |
//! | [`enterprise_auth`]       | key 登录、校验、登出 |
//! | [`enterprise_plugins`]    | 企业插件探测与组合安装 |
//! | [`local_runtime`]         | Profile 物化与 `cordis.patch.yml` 合成 |
//! | [`official_command`]      | 官方命令执行 |
//! | [`owned_process`]         | 子进程登记与清理 |
//! | [`runtime_port`]          | 本地端口分配 |
//! | [`runtime_diagnostics`]   | 运行时诊断 |
//! | [`command_management`]    | 命令管理 |
//! | [`pages`]                 | 入口页与预加载 |
//! | [`browser_worker`]        | 隐藏浏览器 worker（CDP） |
//! | [`plugin_archive_cache`]  | 插件归档缓存 |
//! | [`runtime_compatibility`] | 运行时兼容与撤销记录 |
//! | [`runtime_pack`]          | 离线运行时包校验与修复 |
//!
//! `runtime_pack` keeps a fail-closed contract — never start a host process
//! against a partial tree — while making the failure repairable through an
//! offline archive instead of being fatal.

mod app;
mod browser_worker;
mod datahub_daemon;
mod command_management;
mod connection;
mod deployment_config;
mod enterprise_auth;
mod enterprise_plugins;
mod local_runtime;
mod official_command;
mod owned_process;
mod pages;
mod paths;
mod plugin_archive_cache;
mod runtime_compatibility;
mod runtime_diagnostics;
mod runtime_pack;
mod runtime_port;

pub use app::run;
