import {readFileSync,rmSync} from 'node:fs';
import {resolve} from 'node:path';
const root=process.cwd();const pkg=JSON.parse(readFileSync(resolve(root,'package.json'),'utf8'));
if(!['quantdsh-provider-identity-enterprise','quantdsh-plugin-enterprise-collaboration'].includes(pkg.name))throw new Error('Unexpected enterprise build directory');
rmSync(resolve(root,'dist'),{recursive:true,force:true});
