// readConfig: defaults, required keys and the fail-fast validation of every context value.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { App } from 'aws-cdk-lib';
import { namePrefix, readConfig } from '../lib/config';
import { EXAMPLE_CONTEXT } from './helpers';

const read = (context: Record<string, unknown>) => readConfig(new App({ context }).node);

test('defaults base path and asset directory', () => {
  const { assetDir: _ignored, ...context } = EXAMPLE_CONTEXT;
  const config = read(context);
  assert.equal(config.basePath, '/blog/');
  assert.equal(config.assetDir, '../../../target/lambda');
  assert.equal(config.existingDistribution, undefined);
  assert.equal(config.subdomain, undefined);
  assert.equal(namePrefix(config), 'fp-example-dev');
});

test('reads both distribution modes', () => {
  const mounted = read({ ...EXAMPLE_CONTEXT, distributionId: 'E1EXAMPLE', distributionDomain: 'd1.cloudfront.net' });
  assert.deepEqual(mounted.existingDistribution, { id: 'E1EXAMPLE', domainName: 'd1.cloudfront.net' });
  const sub = read({
    ...EXAMPLE_CONTEXT,
    domainName: 'blog.example.com',
    certificateArn: 'arn:aws:acm:us-east-1:000000000000:certificate/example',
  });
  assert.equal(sub.subdomain?.domainName, 'blog.example.com');
});

test('treats empty strings as absent', () => {
  assert.equal(read({ ...EXAMPLE_CONTEXT, basePath: '' }).basePath, '/blog/');
});

test('rejects missing required keys', () => {
  for (const key of ['site', 'envName', 'alarmEmail', 'mailIdentity']) {
    const context: Record<string, unknown> = { ...EXAMPLE_CONTEXT };
    delete context[key];
    assert.throws(() => read(context), new RegExp(`context ${key} is required`));
  }
});

test('rejects malformed values', () => {
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, site: 'Bad_Name' }), /context site must match/);
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, envName: 'x'.repeat(30) }), /context envName must match/);
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, basePath: '/blog' }), /basePath/);
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, basePath: 'blog/' }), /basePath/);
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, site: 7 }), /context site must be a string/);
});

test('rejects half-specified or conflicting distribution modes', () => {
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, distributionId: 'E1EXAMPLE' }), /go together/);
  assert.throws(() => read({ ...EXAMPLE_CONTEXT, domainName: 'blog.example.com' }), /go together/);
  assert.throws(
    () =>
      read({
        ...EXAMPLE_CONTEXT,
        distributionId: 'E1EXAMPLE',
        distributionDomain: 'd1.cloudfront.net',
        domainName: 'blog.example.com',
        certificateArn: 'arn:aws:acm:us-east-1:000000000000:certificate/example',
      }),
    /not both/,
  );
});
