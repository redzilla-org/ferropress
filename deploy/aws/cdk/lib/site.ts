// Composes the four stacks of one site environment. Stacks are environment-agnostic (no
// account or region), so synthesis needs no credentials and no lookups.
import { App } from 'aws-cdk-lib';
import { namePrefix, SiteConfig } from './config';
import { ComputeStack } from './compute-stack';
import { DataStack } from './data-stack';
import { EdgeStack } from './edge-stack';
import { OpsStack } from './ops-stack';

export interface SiteStacks {
  readonly data: DataStack;
  readonly compute: ComputeStack;
  readonly edge: EdgeStack;
  readonly ops: OpsStack;
}

export function buildSite(app: App, config: SiteConfig): SiteStacks {
  const prefix = namePrefix(config);
  const data = new DataStack(app, `${prefix}-data`, { config });
  const compute = new ComputeStack(app, `${prefix}-compute`, { config, data });
  const edge = new EdgeStack(app, `${prefix}-edge`, { config, data, compute });
  const ops = new OpsStack(app, `${prefix}-ops`, { config, compute, edge });
  return { data, compute, edge, ops };
}
