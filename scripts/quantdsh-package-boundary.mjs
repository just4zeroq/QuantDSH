// Review this inventory when the QuantDSH release adds or removes a package.
// Only product packages are selected or exposed by the default personal Profile.
export const PRODUCT_PACKAGES = Object.freeze([
  'quantdsh-plugin-connectors',
  'quantdsh-plugin-experts',
  'quantdsh-plugin-library',
  'quantdsh-plugin-projects',
  'quantdsh-plugin-skills',
  'quantdsh-plugin-workbenches',
])

// External enterprise packages: installed explicitly, never included in the base installer.
export const ENTERPRISE_PACKAGES = Object.freeze([
  'quantdsh-provider-identity-enterprise',
  'quantdsh-plugin-enterprise-collaboration',
  'quantdsh-enterprise-connection',
])

// Keep the already shipped community catalog integrations on the same Profile.
export const CATALOG_PACKAGES = Object.freeze({
  '@cocofhu/skillhub': '0.2.16',
  dshmarket: '1.66.8',
})

export const RELEASE_PACKAGES = Object.freeze([
  'quantdsh-contracts',
  'quantdsh-ui',
  'quantdsh-provider-identity-local',
  'quantdsh-provider-browser-session',
  'quantdsh-plugin-audit',
  'quantdsh-plugin-access',
  'quantdsh-plugin-skills',
  'quantdsh-plugin-experts',
  'quantdsh-plugin-connectors',
  'quantdsh-plugin-library',
  'quantdsh-bundle',
  'quantdsh-plugin-projects',
  'quantdsh-plugin-workbenches',
])

export const PACKAGE_DIRECTORIES = Object.freeze({
  'quantdsh-contracts': '../../packages/contracts',
  'quantdsh-ui': '../../packages/ui',
  'quantdsh-provider-identity-local': '../../packages/providers/identity-local',
  'quantdsh-provider-browser-session': '../../packages/providers/browser-session',
  'quantdsh-plugin-audit': '../../packages/plugins/audit',
  'quantdsh-plugin-access': '../../packages/plugins/access',
  'quantdsh-plugin-skills': '../../packages/plugins/skills',
  'quantdsh-plugin-experts': '../../packages/plugins/experts',
  'quantdsh-plugin-connectors': '../../packages/plugins/connectors',
  'quantdsh-plugin-library': '../../packages/plugins/library',
  'quantdsh-bundle': '../../packages/bundle',
  'quantdsh-plugin-projects': '../../packages/plugins/projects',
  'quantdsh-plugin-workbenches': '../../packages/plugins/workbenches',
  'quantdsh-provider-identity-enterprise': '../../packages/providers/identity-enterprise',
  'quantdsh-plugin-enterprise-collaboration': '../../packages/plugins/enterprise-collaboration',
})
