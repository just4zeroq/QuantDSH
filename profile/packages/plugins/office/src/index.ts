import type { Context } from "@deepseek-ai/cordis";
import { ContentService } from "./content/service.js";
import * as Tools from "./content/tools.js";
import * as Connection from "./content/connection.js";
export const name = "quantdsh-office";
// The Loader entry must declare the services its child modules consume, too.
export const inject = [
  "fs",
  "storageDomain",
  "tools",
  "systemPrompt",
  "connection",
  "quantdshIdentity",
  "quantdshAccess",
  "quantdshAudit",
];
export interface Config {
  tools?: boolean;
}
export async function apply(ctx: Context, config: Config = {}) {
  await ctx.plugin(ContentService);
  await ctx.plugin(Connection);
  if (config.tools !== false) await ctx.plugin(Tools);
}
