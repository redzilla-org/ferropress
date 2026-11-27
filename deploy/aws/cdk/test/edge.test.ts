// Edge stack: the fp-public cache policy, the section 6 behavior table in both mount modes,
// OAC permissions scoped to one distribution, and the CloudFront Function behavior.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import { Match, Template } from 'aws-cdk-lib/assertions';
import { routerCode, uploadsRewriteCode } from '../lib/edge-functions';
import { behaviorTable, PUBLIC_QUERY_ALLOWLIST } from '../lib/edge';
import { URL_FUNCTIONS } from '../lib/functions';
import { flat, resources, site } from './helpers';

const created = Template.fromStack(site().edge);
const ALL_VIEWER_EXCEPT_HOST = cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER.originRequestPolicyId;
const CACHING_DISABLED = cloudfront.CachePolicy.CACHING_DISABLED.cachePolicyId;
const CACHING_OPTIMIZED = cloudfront.CachePolicy.CACHING_OPTIMIZED.cachePolicyId;

function distributionConfig(template: Template): any {
  const [distribution] = Object.values(resources(template, 'AWS::CloudFront::Distribution'));
  return distribution.Properties.DistributionConfig;
}

test('fp-public caches by path and the query allowlist, no cookies, gzip and br, 0/1d/1y', () => {
  created.hasResourceProperties('AWS::CloudFront::CachePolicy', {
    CachePolicyConfig: {
      Name: 'fp-public-example-dev',
      MinTTL: 0,
      DefaultTTL: 86400,
      MaxTTL: 31536000,
      ParametersInCacheKeyAndForwardedToOrigin: {
        QueryStringsConfig: { QueryStringBehavior: 'whitelist', QueryStrings: PUBLIC_QUERY_ALLOWLIST },
        CookiesConfig: { CookieBehavior: 'none' },
        HeadersConfig: { HeaderBehavior: 'none' },
        EnableAcceptEncodingGzip: true,
        EnableAcceptEncodingBrotli: true,
      },
    },
  });
  assert.deepEqual(PUBLIC_QUERY_ALLOWLIST, ['p', 'page_id', 'paged', 'cat', 'tag', 'author', 'm', 'feed', 's']);
});

test('behavior table matches section 6 in precedence order', () => {
  assert.deepEqual(
    behaviorTable('/blog/').map((r) => [r.pattern, r.origin, r.kind]),
    [
      ['blog/wp-content/uploads/*', 'uploads', 'media'],
      ['blog/api/media/sign', 'media-sign', 'dynamic'],
      ['blog/admin/*', 'admin', 'dynamic'],
      ['blog/api/*', 'admin', 'dynamic'],
      ['blog/login*', 'auth', 'dynamic'],
      ['blog/comments*', 'comments', 'dynamic'],
      ['blog/*', 'read', 'public'],
      ['blog', 'read', 'bare'],
    ],
  );
  assert.deepEqual(
    behaviorTable('/').map((r) => r.pattern),
    ['wp-content/uploads/*', 'api/media/sign', 'admin/*', 'api/*', 'login*', 'comments*', '*'],
  );
});

test('created distribution carries every behavior with its policies', () => {
  const config = distributionConfig(created);
  const behaviors: any[] = config.CacheBehaviors;
  assert.deepEqual(
    behaviors.map((b) => b.PathPattern),
    behaviorTable('/blog/').map((r) => r.pattern),
  );
  const byPattern = Object.fromEntries(behaviors.map((b) => [b.PathPattern, b]));
  const uploads = byPattern['blog/wp-content/uploads/*'];
  assert.equal(uploads.CachePolicyId, CACHING_OPTIMIZED);
  assert.deepEqual(uploads.AllowedMethods, ['GET', 'HEAD']);
  for (const pattern of ['blog/api/media/sign', 'blog/admin/*', 'blog/api/*', 'blog/login*', 'blog/comments*']) {
    assert.equal(byPattern[pattern].CachePolicyId, CACHING_DISABLED, pattern);
    assert.equal(byPattern[pattern].OriginRequestPolicyId, ALL_VIEWER_EXCEPT_HOST, pattern);
    assert.equal(byPattern[pattern].AllowedMethods.length, 7, pattern);
  }
  const pub = byPattern['blog/*'];
  assert.match(flat(pub.CachePolicyId), /PublicCachePolicy/);
  assert.equal(pub.OriginRequestPolicyId, ALL_VIEWER_EXCEPT_HOST);
  assert.deepEqual(pub.AllowedMethods, ['GET', 'HEAD']);
  assert.match(flat(pub.FunctionAssociations), /Router/);
  assert.equal(byPattern.blog.CachePolicyId, CACHING_DISABLED);
  assert.match(flat(byPattern.blog.FunctionAssociations), /Router/);
  for (const b of behaviors) {
    assert.equal(b.ViewerProtocolPolicy, 'redirect-to-https', b.PathPattern);
  }
  // The default behavior serves public pages exactly like blog/*.
  assert.match(flat(config.DefaultCacheBehavior.CachePolicyId), /PublicCachePolicy/);
  assert.equal(config.Aliases, undefined);
});

test('origins sit behind OAC: function URLs over HTTPS and the bucket site/ prefix', () => {
  created.resourceCountIs('AWS::CloudFront::OriginAccessControl', 2);
  created.hasResourceProperties('AWS::CloudFront::OriginAccessControl', {
    OriginAccessControlConfig: Match.objectLike({ OriginAccessControlOriginType: 'lambda', SigningBehavior: 'always' }),
  });
  created.hasResourceProperties('AWS::CloudFront::OriginAccessControl', {
    OriginAccessControlConfig: Match.objectLike({ OriginAccessControlOriginType: 's3', SigningBehavior: 'always' }),
  });
  const origins: any[] = distributionConfig(created).Origins;
  // read, admin, auth, media-sign, comments and uploads; search is selected by the router.
  assert.equal(origins.length, 6);
  for (const origin of origins) {
    assert.ok(origin.OriginAccessControlId, origin.Id);
    if (origin.S3OriginConfig !== undefined) {
      assert.equal(origin.OriginPath, '/site');
    } else {
      assert.equal(origin.CustomOriginConfig.OriginProtocolPolicy, 'https-only');
    }
  }
});

test('CloudFront alone may invoke each URL and read media, scoped to this distribution', () => {
  const permissions = Object.values(resources(created, 'AWS::Lambda::Permission'));
  assert.equal(permissions.length, URL_FUNCTIONS.length * 2);
  for (const p of permissions) {
    assert.equal(p.Properties.Principal, 'cloudfront.amazonaws.com');
    assert.match(flat(p.Properties.SourceArn), /distribution\/.*Distribution/);
  }
  assert.equal(permissions.filter((p) => p.Properties.Action === 'lambda:InvokeFunctionUrl').length, 6);
  assert.equal(permissions.filter((p) => p.Properties.InvokedViaFunctionUrl === true).length, 6);

  created.resourceCountIs('AWS::S3::BucketPolicy', 1);
  const [policy] = Object.values(resources(created, 'AWS::S3::BucketPolicy'));
  const statements: any[] = policy.Properties.PolicyDocument.Statement;
  assert.deepEqual(statements.map((s) => [s.Sid, s.Effect]), [
    ['TlsOnly', 'Deny'],
    ['CloudFrontReadsMedia', 'Allow'],
  ]);
  assert.equal(statements[1].Action, 's3:GetObject');
  assert.match(flat(statements[1].Resource), /site\/uploads\/\*/);
  assert.match(flat(statements[1].Condition), /Distribution/);
});

test('projector may invalidate only this distribution, found through SSM', () => {
  created.hasResourceProperties('AWS::SSM::Parameter', {
    Name: '/ferropress/example/dev/distribution-id',
    Type: 'String',
  });
  const [policy] = Object.values(resources(created, 'AWS::IAM::Policy'));
  created.resourceCountIs('AWS::IAM::Policy', 1);
  assert.equal(policy.Properties.PolicyDocument.Statement.length, 1);
  const statement = policy.Properties.PolicyDocument.Statement[0];
  assert.equal(statement.Action, 'cloudfront:CreateInvalidation');
  assert.match(flat(statement.Resource), /distribution\/.*Distribution/);
  assert.match(flat(policy.Properties.Roles), /ProjectRole/);
});

test('response headers: HSTS, nosniff and referrer policy; admin adds frame-ancestors', () => {
  created.resourceCountIs('AWS::CloudFront::ResponseHeadersPolicy', 2);
  for (const policy of Object.values(resources(created, 'AWS::CloudFront::ResponseHeadersPolicy'))) {
    const security = policy.Properties.ResponseHeadersPolicyConfig.SecurityHeadersConfig;
    assert.equal(security.StrictTransportSecurity.AccessControlMaxAgeSec, 63072000);
    assert.ok(security.ContentTypeOptions);
    assert.equal(security.ReferrerPolicy.ReferrerPolicy, 'strict-origin-when-cross-origin');
  }
  created.hasResourceProperties('AWS::CloudFront::ResponseHeadersPolicy', {
    ResponseHeadersPolicyConfig: Match.objectLike({
      Name: 'fp-example-dev-admin-headers',
      SecurityHeadersConfig: Match.objectLike({
        ContentSecurityPolicy: { ContentSecurityPolicy: "frame-ancestors 'self'", Override: true },
      }),
    }),
  });
  const behaviors: any[] = distributionConfig(created).CacheBehaviors;
  for (const b of behaviors) {
    const admin = /\/(admin|api|login)/.test(b.PathPattern);
    assert.match(flat(b.ResponseHeadersPolicyId), admin ? /AdminHeaders/ : /PublicHeaders/, b.PathPattern);
  }
});

test('subdomain mode sets the alias, certificate and TLS 1.2 policy', () => {
  const template = Template.fromStack(
    site({
      domainName: 'blog.example.com',
      certificateArn: 'arn:aws:acm:us-east-1:000000000000:certificate/example',
      basePath: '/',
    }).edge,
  );
  const config = distributionConfig(template);
  assert.deepEqual(config.Aliases, ['blog.example.com']);
  assert.equal(config.ViewerCertificate.MinimumProtocolVersion, 'TLSv1.2_2021');
  // At the root the public row is the default behavior and there is no bare-path redirect row.
  assert.deepEqual(
    config.CacheBehaviors.map((b: any) => b.PathPattern),
    ['wp-content/uploads/*', 'api/media/sign', 'admin/*', 'api/*', 'login*', 'comments*'],
  );
});

test('existing-distribution mode creates no distribution and exports the manifest', () => {
  const template = Template.fromStack(
    site({ distributionId: 'E1EXAMPLE', distributionDomain: 'd1.cloudfront.net' }).edge,
  );
  template.resourceCountIs('AWS::CloudFront::Distribution', 0);
  template.resourceCountIs('AWS::CloudFront::CachePolicy', 1);
  template.resourceCountIs('AWS::CloudFront::Function', 2);
  for (const p of Object.values(resources(template, 'AWS::Lambda::Permission'))) {
    assert.match(flat(p.Properties.SourceArn), /distribution\/E1EXAMPLE/);
  }
  template.hasOutput('DistributionId', { Value: 'E1EXAMPLE' });
  const manifest = flat(template.findOutputs('BehaviorManifest'));
  for (const row of behaviorTable('/blog/')) {
    assert.ok(manifest.includes(`\\"pathPattern\\":\\"${row.pattern}\\"`), row.pattern);
  }
  assert.ok(manifest.includes(ALL_VIEWER_EXCEPT_HOST));
});

// CloudFront Functions run in cloudfront-js-2.0; here they run in Node with a fake `cf` module.
function load(code: string, cf: object = {}): (event: any) => any {
  const body = code.replace("import cf from 'cloudfront';", '');
  return new Function('cf', `${body}\nreturn handler;`)(cf) as (event: any) => any;
}

test('router redirects the bare base path, keeping the query string', () => {
  const handler = load(routerCode('/blog/', 'search.example.com'));
  const response = handler({
    request: { uri: '/blog', querystring: { s: { value: 'x' }, tag: { multiValue: [{ value: 'a' }, { value: '' }] } } },
  });
  assert.equal(response.statusCode, 301);
  assert.equal(response.headers.location.value, '/blog/?s=x&tag=a&tag');
  const plain = handler({ request: { uri: '/blog', querystring: {} } });
  assert.equal(plain.headers.location.value, '/blog/');
});

test('router sends query s to the search origin with OAC signing and passes others through', () => {
  const calls: any[] = [];
  const handler = load(routerCode('/blog/', 'search.example.com'), { updateRequestOrigin: (o: any) => calls.push(o) });
  const request = { uri: '/blog/', querystring: { s: { value: 'condo' } } };
  assert.equal(handler({ request }), request);
  assert.deepEqual(calls, [
    {
      domainName: 'search.example.com',
      originAccessControlConfig: { enabled: true, signingBehavior: 'always', signingProtocol: 'sigv4', originType: 'lambda' },
    },
  ]);
  const other = { uri: '/blog/2026/11/post/', querystring: { paged: { value: '2' } } };
  assert.equal(handler({ request: other }), other);
  assert.equal(calls.length, 1);
});

test('router at the root never redirects', () => {
  const handler = load(routerCode('/', 'search.example.com'));
  const request = { uri: '/', querystring: {} };
  assert.equal(handler({ request }), request);
});

test('uploads rewrite maps the public path onto site/uploads', () => {
  const handler = load(uploadsRewriteCode('/blog/'));
  assert.equal(handler({ request: { uri: '/blog/wp-content/uploads/2026/11/a.jpg' } }).uri, '/uploads/2026/11/a.jpg');
  assert.equal(handler({ request: { uri: '/elsewhere.jpg' } }).uri, '/elsewhere.jpg');
});
