import type { Context } from '@deepseek-ai/cordis';
import type { WorkbenchDescriptor } from 'quantdsh-contracts';
import type { ClientWorkbenchRegistration } from 'quantdsh-plugin-workbenches/client';
import { MarketPage } from './MarketPage';

export const name = 'quantdsh-market-client';
export const inject = ['quantdshWorkbenchClientRegistration', 'layout'];

const descriptor: WorkbenchDescriptor = {
  id: 'quantdsh.market',
  title: '行情',
  description: 'K 线行情看盘（Vela 图表，数据源为 quantdsh-datahub）',
  protocolVersion: '1.0',
  layout: 'full-page',
  hostCompatibility: { sdk: '^1.0.0', harness: '0.2.1-rc.2' },
  capabilities: [],
};

export async function apply(ctx: Context) {
  const registration = ctx.get('quantdshWorkbenchClientRegistration') as ClientWorkbenchRegistration<React.ComponentType>;
  await registration.registerManaged(ctx, descriptor, () => <MarketPage nowTs={Date.now()} />);
}
