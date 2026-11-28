// Shared test setup: a synthesized site environment from example context and the committed
// asset fixture, so no test needs the Rust build or AWS credentials.
import { App } from 'aws-cdk-lib';
import { Template } from 'aws-cdk-lib/assertions';
import { readConfig } from '../lib/config';
import { buildSite, SiteStacks } from '../lib/site';

export const EXAMPLE_CONTEXT: Record<string, string> = {
  site: 'example',
  envName: 'dev',
  alarmEmail: 'alerts@example.com',
  mailIdentity: 'example.com',
  assetDir: 'test/fixtures/lambda',
  '@aws-cdk/core:defaultCrossStackReferences': 'strong',
};

export function site(overrides: Record<string, string> = {}): SiteStacks {
  const app = new App({ context: { ...EXAMPLE_CONTEXT, ...overrides } });
  return buildSite(app, readConfig(app.node));
}

/** Logical id -> resource for one resource type. */
export function resources(template: Template, type: string): Record<string, any> {
  return template.findResources(type);
}

/** Everything in a CloudFormation value rendered to one string, for substring checks on intrinsics. */
export function flat(value: unknown): string {
  return JSON.stringify(value);
}
