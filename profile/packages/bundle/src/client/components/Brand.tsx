import * as React from 'react';
import { LogoMark } from 'quantdsh-ui';

export function BrandName() {
  return <span data-testid="quantdsh-brand">QuantDSH</span>;
}

export function BrandMark() {
  return <LogoMark size={22} />;
}

export function DiagnosticsMark() {
  return <span aria-hidden>W</span>;
}
