// Compute stack: the section 3 function table, per-function least-privilege IAM, triggers,
// destinations, logs and the salts secret.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Match, Template } from 'aws-cdk-lib/assertions';
import { FUNCTIONS, FunctionName } from '../lib/functions';
import { flat, resources, site } from './helpers';

const stacks = site();
const template = Template.fromStack(stacks.compute);

const pascal = (name: string) =>
  name
    .split('-')
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join('');

// Resource hints, most specific first: each granted resource must match exactly one label.
const HINTS: [string, string][] = [
  ['site/rendered/*', 'rendered'],
  ['site/fragments/*', 'fragments'],
  ['site/search/*', 'search-objects'],
  ['site/incoming/*', 'incoming'],
  ['site/uploads/*', 'uploads'],
  ['site/imports/*', 'imports'],
  ['/index/user_id', 'sessions-index'],
  ['ReadTable', 'read-table'],
  ['SessionsTable', 'sessions'],
  ['LimitsTable', 'limits'],
  ['Cluster', 'cluster'],
  ['SiteSecret', 'secret'],
  ['ProjectFunction', 'project'],
  ['identity/example.com', 'mail'],
  ['parameter/ferropress/example/dev/distribution-id', 'dist-param'],
  ['FailureQueue', 'failure-queue'],
];

// Expected grants per function as "action label"; every function also writes its own logs.
const EXPECTED: Record<FunctionName, string[]> = {
  read: ['dynamodb:GetItem read-table', 's3:GetObject rendered', 's3:GetObject fragments'],
  admin: [
    'dsql:DbConnect cluster',
    'dynamodb:GetItem sessions',
    'lambda:InvokeFunction project',
    'secretsmanager:GetSecretValue secret',
  ],
  auth: [
    'dsql:DbConnect cluster',
    'dynamodb:PutItem sessions',
    'dynamodb:DeleteItem sessions',
    'dynamodb:GetItem sessions',
    'dynamodb:Query sessions-index',
    'dynamodb:UpdateItem limits',
    'ses:SendEmail mail',
    'secretsmanager:GetSecretValue secret',
  ],
  'media-sign': ['s3:PutObject incoming', 'dynamodb:GetItem sessions'],
  'media-process': [
    's3:GetObject incoming',
    's3:DeleteObject incoming',
    's3:PutObject uploads',
    'dsql:DbConnect cluster',
    'lambda:InvokeFunction project',
    'sqs:SendMessage failure-queue',
  ],
  search: ['dynamodb:GetItem read-table', 's3:GetObject search-objects'],
  comments: [
    'dsql:DbConnect cluster',
    'dynamodb:UpdateItem limits',
    'lambda:InvokeFunction project',
    'secretsmanager:GetSecretValue secret',
  ],
  project: [
    'dsql:DbConnect cluster',
    's3:PutObject rendered',
    's3:PutObject fragments',
    's3:PutObject search-objects',
    'dynamodb:PutItem read-table',
    'dynamodb:DeleteItem read-table',
    'dynamodb:Query read-table',
    'ssm:GetParameter dist-param',
    'sqs:SendMessage failure-queue',
  ],
  cron: ['dsql:DbConnect cluster', 'lambda:InvokeFunction project', 'sqs:SendMessage failure-queue'],
  import: [
    's3:GetObject imports',
    's3:PutObject uploads',
    'dsql:DbConnect cluster',
    'dynamodb:GetItem read-table',
    'dynamodb:PutItem read-table',
    'lambda:InvokeFunction project',
    'sqs:SendMessage failure-queue',
  ],
};

function functionResource(name: FunctionName): any {
  const all = resources(template, 'AWS::Lambda::Function');
  const id = Object.keys(all).find((k) => k.startsWith(`${pascal(name)}Function`));
  assert.ok(id, `function ${name}`);
  return all[id];
}

function grantsOf(name: FunctionName): string[] {
  const roleId: string = functionResource(name).Properties.Role['Fn::GetAtt'][0];
  const role = resources(template, 'AWS::IAM::Role')[roleId];
  assert.equal(role.Properties.ManagedPolicyArns, undefined, `${name} has no managed policies`);
  const policies = Object.values(resources(template, 'AWS::IAM::Policy')).filter((p) =>
    p.Properties.Roles.some((r: any) => r.Ref === roleId),
  );
  assert.equal(policies.length, 1, `${name} has one inline policy`);
  const grants: string[] = [];
  for (const statement of policies[0].Properties.PolicyDocument.Statement) {
    assert.equal(statement.Effect, 'Allow');
    assert.equal(statement.Condition, undefined);
    const actions: string[] = [statement.Action].flat();
    const targets: unknown[] = [statement.Resource].flat();
    for (const action of actions) {
      for (const target of targets) {
        const text = flat(target);
        if (text.includes(`${pascal(name)}Logs`) && action.startsWith('logs:')) {
          grants.push(`${action} own-logs`);
          continue;
        }
        const hint = HINTS.find(([needle]) => text.includes(needle));
        grants.push(`${action} ${hint ? hint[1] : `UNSCOPED ${text}`}`);
      }
    }
  }
  return grants.sort();
}

test('one function per section 3 row: arm64, provided.al2023, memory and timeout', () => {
  template.resourceCountIs('AWS::Lambda::Function', FUNCTIONS.length);
  assert.equal(FUNCTIONS.length, 10);
  for (const spec of FUNCTIONS) {
    const props = functionResource(spec.name).Properties;
    assert.deepEqual(props.Architectures, ['arm64'], spec.name);
    assert.equal(props.Runtime, 'provided.al2023', spec.name);
    assert.equal(props.Handler, 'bootstrap', spec.name);
    assert.equal(props.MemorySize, spec.memoryMb, spec.name);
    assert.equal(props.Timeout, spec.timeoutSeconds, spec.name);
    assert.equal(props.VpcConfig, undefined, `${spec.name} runs outside a VPC`);
  }
});

test('each function has its own role holding exactly its section 3 grants', () => {
  template.resourceCountIs('AWS::IAM::Role', FUNCTIONS.length + 2); // + two scheduler roles
  const roles = new Set(FUNCTIONS.map((s) => functionResource(s.name).Properties.Role['Fn::GetAtt'][0]));
  assert.equal(roles.size, FUNCTIONS.length);
  for (const spec of FUNCTIONS) {
    const expected = [...EXPECTED[spec.name], 'logs:CreateLogStream own-logs', 'logs:PutLogEvents own-logs'];
    assert.deepEqual(grantsOf(spec.name), expected.sort(), spec.name);
  }
});

test('function URLs use AWS_IAM for exactly the CloudFront-facing functions', () => {
  const urlNames = FUNCTIONS.filter((f) => f.url).map((f) => f.name);
  assert.deepEqual(urlNames, ['read', 'admin', 'auth', 'media-sign', 'search', 'comments']);
  template.resourceCountIs('AWS::Lambda::Url', urlNames.length);
  for (const url of Object.values(resources(template, 'AWS::Lambda::Url'))) {
    assert.equal(url.Properties.AuthType, 'AWS_IAM');
  }
  // No permission opens a URL to the public; the Edge stack grants CloudFront only.
  template.resourceCountIs('AWS::Lambda::Permission', 2);
});

test('project runs alone and async functions fail over to the SQS queue', () => {
  assert.equal(functionResource('project').Properties.ReservedConcurrentExecutions, 1);
  for (const spec of FUNCTIONS.filter((f) => f.name !== 'project')) {
    assert.equal(functionResource(spec.name).Properties.ReservedConcurrentExecutions, undefined, spec.name);
  }
  template.resourceCountIs('AWS::SQS::Queue', 1);
  const configs = Object.values(resources(template, 'AWS::Lambda::EventInvokeConfig'));
  assert.equal(configs.length, FUNCTIONS.filter((f) => f.async).length);
  for (const config of configs) {
    assert.match(flat(config.Properties.DestinationConfig.OnFailure.Destination), /FailureQueue/);
  }
});

test('schedules drive project every 5 minutes and cron every minute', () => {
  template.resourceCountIs('AWS::Scheduler::Schedule', 2);
  template.hasResourceProperties('AWS::Scheduler::Schedule', {
    ScheduleExpression: 'rate(5 minutes)',
    Target: Match.objectLike({ Arn: { 'Fn::GetAtt': [Match.stringLikeRegexp('^ProjectFunction'), 'Arn'] } }),
  });
  template.hasResourceProperties('AWS::Scheduler::Schedule', {
    ScheduleExpression: 'rate(1 minute)',
    Target: Match.objectLike({ Arn: { 'Fn::GetAtt': [Match.stringLikeRegexp('^CronFunction'), 'Arn'] } }),
  });
});

test('object-created events on incoming/ and imports/ reach media-process and import', () => {
  template.resourceCountIs('AWS::Events::Rule', 2);
  for (const [prefix, fn] of [
    ['site/incoming/', 'MediaProcessFunction'],
    ['site/imports/', 'ImportFunction'],
  ]) {
    template.hasResourceProperties('AWS::Events::Rule', {
      EventPattern: Match.objectLike({
        source: ['aws.s3'],
        'detail-type': ['Object Created'],
        detail: Match.objectLike({ object: { key: [{ prefix }] } }),
      }),
      Targets: [Match.objectLike({ Arn: { 'Fn::GetAtt': [Match.stringLikeRegexp(`^${fn}`), 'Arn'] } })],
    });
  }
});

test('logs are kept 30 days in one group per function', () => {
  template.resourceCountIs('AWS::Logs::LogGroup', FUNCTIONS.length);
  for (const group of Object.values(resources(template, 'AWS::Logs::LogGroup'))) {
    assert.equal(group.Properties.RetentionInDays, 30);
  }
});

test('salts live in a generated, retained secret and never in the environment', () => {
  template.resourceCountIs('AWS::SecretsManager::Secret', 1);
  const [id, secret] = Object.entries(resources(template, 'AWS::SecretsManager::Secret'))[0];
  assert.equal(secret.DeletionPolicy, 'Retain');
  assert.ok(secret.Properties.GenerateSecretString);
  assert.equal(secret.Properties.SecretString, undefined);
  for (const spec of FUNCTIONS) {
    const env = functionResource(spec.name).Properties.Environment.Variables;
    const text = flat(env);
    assert.ok(!text.includes('resolve:secretsmanager'), spec.name);
    // Only the ARN is passed, and only to the functions allowed to read it.
    const reads = EXPECTED[spec.name].some((g) => g.endsWith(' secret'));
    assert.equal(env.FP_SECRET_ARN === undefined ? false : flat(env.FP_SECRET_ARN).includes(id), reads, spec.name);
  }
});

test('environment names exactly the resources each role may touch', () => {
  const env = (name: FunctionName) => Object.keys(functionResource(name).Properties.Environment.Variables).sort();
  assert.deepEqual(env('read'), ['FP_BASE_PATH', 'FP_BUCKET', 'FP_READ_TABLE', 'FP_SITE']);
  assert.deepEqual(env('project'), [
    'FP_BASE_PATH',
    'FP_BUCKET',
    'FP_DISTRIBUTION_PARAM',
    'FP_DSQL_ENDPOINT',
    'FP_READ_TABLE',
    'FP_SITE',
  ]);
  assert.deepEqual(env('auth'), [
    'FP_BASE_PATH',
    'FP_DSQL_ENDPOINT',
    'FP_LIMITS_TABLE',
    'FP_MAIL_IDENTITY',
    'FP_SECRET_ARN',
    'FP_SESSIONS_TABLE',
    'FP_SITE',
  ]);
  assert.deepEqual(env('cron'), ['FP_BASE_PATH', 'FP_DSQL_ENDPOINT', 'FP_PROJECT_FUNCTION', 'FP_SITE']);
});

test('code comes from <assetDir>/lambda-<name>', () => {
  // Each fixture directory is distinct, so each function gets its own content-hashed asset.
  const keys = new Set(FUNCTIONS.map((spec) => flat(functionResource(spec.name).Properties.Code.S3Key)));
  assert.equal(keys.size, FUNCTIONS.length);
  // A missing asset directory fails synthesis instead of deploying an empty function.
  assert.throws(() => site({ assetDir: 'test/fixtures/missing' }), /Cannot find asset/);
});
