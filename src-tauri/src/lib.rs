//! QuantDSH — a 100% functional Tauri port of the Electron desktop carrier
//! (`workdsh/apps/desktop/src/workdsh-main.ts`).
//!
//! The module layout mirrors the TypeScript source one-to-one:
//!
//! | TypeScript                | Rust                     |
//! |---------------------------|--------------------------|
//! | `workdsh-main.ts`         | [`app`]                  |
//! | path helpers in `workdsh-main.ts` | [`paths`]        |
//! | `connection-mode.ts`      | [`connection`]           |
//! | `deployment-config.ts`    | [`deployment_config`]    |
//! | `enterprise-auth.ts`      | [`enterprise_auth`]      |
//! | `enterprise-plugins.ts`   | [`enterprise_plugins`]   |
//! | `local-runtime.ts`        | [`local_runtime`]        |
//! | `official-command.ts`     | [`official_command`]     |
//! | `owned-process.ts`        | [`owned_process`]        |
//! | `runtime-port.ts`         | [`runtime_port`]         |
//! | `runtime-diagnostics.ts`  | [`runtime_diagnostics`]  |
//! | `command-management.ts`   | [`command_management`]   |
//! | `connection-entry.ts` + `connection-preload.ts` | [`pages`] |
//! | `startBrowserWorker` in `workdsh-main.ts` | [`browser_worker`] |
//! | `command-cli.ts`          | `runtime-cli/command-cli.mjs` (outside the crate) |
//! | `plugin-archive-cache.ts` | [`plugin_archive_cache`] |
//! | `runtime-compatibility.ts`| [`runtime_compatibility`]|
//! | — (Tauri-only)            | [`runtime_pack`]         |
//!
//! `runtime_pack` has no TypeScript counterpart: the Electron carrier treats a
//! damaged runtime as fatal (`Bundled Node.js runtime is missing`). It keeps the
//! same fail-closed contract — never start a host process against a partial tree
//! — while making the failure repairable through an offline archive.

mod app;
mod browser_worker;
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
