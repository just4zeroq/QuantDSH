import type { Context } from '@deepseek-ai/cordis';
import type {} from '@deepseek-ai/dsh-client-ui-slots';
import type {} from '@deepseek-ai/dsh-client-ui-renderer/client';
import type {} from '@deepseek-ai/dsh-client-ui-sidebar/client';
import type {} from '@deepseek-ai/dsh-client-ui-layout/client';
import { DataWorkbench } from './client/DataWorkbench.js';

export const name = 'quantdsh-data-client';
export const inject = ['slots', 'layout'];

/** 侧栏直达入口。数据工坊是内置业务页，不经过工作台注册链：
 * 页面与插件包是同一个身份（加载即信任），不存在"另一个可替换的包"需要对账。 */
const DataIcon = ({ size = 20 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} strokeLinecap="round" strokeLinejoin="round" aria-hidden focusable={false}>
    <ellipse cx="12" cy="6" rx="8" ry="3" />
    <path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6" />
    <path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
  </svg>
);

/** 面板 key 与侧栏 id 必须一致：点击侧栏项时布局按该 id 选择面板。 */
const DATA_PANEL = 'quantdsh-data';

export function apply(ctx: Context) {
  ctx.slots.inject('main', () => ctx.slots.register({ name: 'main', key: DATA_PANEL }, () => (
    <div style={{ height: '100%', minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <DataWorkbench nowTs={Date.now()} />
    </div>
  )));
  // order -10：侧栏由 `@deepseek-ai/dsh-client-ui-plugin-manager` 的「插件」项占 order 0
  // （定时任务 10、项目 20、专家 30、协作 35、工作台 44）。要让数据工坊排在「插件」之上，
  // 必须是负数 —— 数字最小者最先渲染。
  ctx.slots.inject('sidebar.panellist', () => ctx.slots.register(
    { name: 'sidebar.panellist', id: DATA_PANEL, label: '数据工坊', order: -10 },
    ({ size }: { size: number }) => <DataIcon size={size} />,
  ));
}
