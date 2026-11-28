// Site-environment parameters read from CDK context. Every stack takes the same config so one
// app synthesizes one site environment; nothing here needs an AWS account lookup.
import { Node } from 'constructs';

/** An existing CloudFront distribution the blog mounts on at its base path. */
export interface ExistingDistribution {
  readonly id: string;
  readonly domainName: string;
}

/** A subdomain distribution the Edge stack creates. */
export interface SubdomainDistribution {
  readonly domainName: string;
  readonly certificateArn: string;
}

export interface SiteConfig {
  /** Short site name, used in resource names (lowercase letters, digits, dashes). */
  readonly site: string;
  /** Environment name such as `dev` or `prod`, used in resource names. */
  readonly envName: string;
  /** Public base path with leading and trailing slash, `/blog/` by default. */
  readonly basePath: string;
  /** Mount on this distribution instead of creating one. */
  readonly existingDistribution?: ExistingDistribution;
  /** Alternate domain for a created distribution. */
  readonly subdomain?: SubdomainDistribution;
  /** Alarm notifications go to this address. */
  readonly alarmEmail: string;
  /** SES identity (domain or address) that `auth` may send mail from. */
  readonly mailIdentity: string;
  /** Directory holding `<binary>/bootstrap` for every function, built by CI. */
  readonly assetDir: string;
}

// The default asset layout is cargo-lambda's `target/lambda/<binary>/bootstrap` at the repo root.
const DEFAULT_ASSET_DIR = '../../../target/lambda';
const NAME = /^[a-z][a-z0-9-]{0,23}$/;

// Context values arrive as strings or are absent; empty strings count as absent.
function optional(node: Node, key: string): string | undefined {
  const value: unknown = node.tryGetContext(key);
  if (value === undefined || value === null || value === '') {
    return undefined;
  }
  if (typeof value !== 'string') {
    throw new Error(`context ${key} must be a string`);
  }
  return value;
}

function required(node: Node, key: string): string {
  const value = optional(node, key);
  if (value === undefined) {
    throw new Error(`context ${key} is required`);
  }
  return value;
}

// Fail at synth on any malformed value rather than producing a template that half-works.
export function readConfig(node: Node): SiteConfig {
  const site = required(node, 'site');
  const envName = required(node, 'envName');
  for (const [key, value] of [['site', site], ['envName', envName]]) {
    if (!NAME.test(value)) {
      throw new Error(`context ${key} must match ${NAME.source}`);
    }
  }
  const basePath = optional(node, 'basePath') ?? '/blog/';
  if (!/^\/([a-z0-9-]+\/)*$/.test(basePath)) {
    throw new Error('context basePath must be "/" or "/segment/.../"');
  }

  const distributionId = optional(node, 'distributionId');
  const distributionDomain = optional(node, 'distributionDomain');
  if ((distributionId === undefined) !== (distributionDomain === undefined)) {
    throw new Error('context distributionId and distributionDomain go together');
  }
  const domainName = optional(node, 'domainName');
  const certificateArn = optional(node, 'certificateArn');
  if ((domainName === undefined) !== (certificateArn === undefined)) {
    throw new Error('context domainName and certificateArn go together');
  }
  if (distributionId !== undefined && domainName !== undefined) {
    throw new Error('mount on an existing distribution or create a subdomain one, not both');
  }

  return {
    site,
    envName,
    basePath,
    existingDistribution:
      distributionId !== undefined && distributionDomain !== undefined
        ? { id: distributionId, domainName: distributionDomain }
        : undefined,
    subdomain:
      domainName !== undefined && certificateArn !== undefined
        ? { domainName, certificateArn }
        : undefined,
    alarmEmail: required(node, 'alarmEmail'),
    mailIdentity: required(node, 'mailIdentity'),
    assetDir: optional(node, 'assetDir') ?? DEFAULT_ASSET_DIR,
  };
}

/** Prefix for physical names that must be unique per account, e.g. `fp-example-dev`. */
export function namePrefix(config: SiteConfig): string {
  return `fp-${config.site}-${config.envName}`;
}
