// Data stack: resource counts, table shapes, bucket hardening and lifecycle, retain policies.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Match, Template } from 'aws-cdk-lib/assertions';
import { resources, site } from './helpers';

const template = Template.fromStack(site().data);

test('holds exactly the stateful resources and no bucket policy', () => {
  template.resourceCountIs('AWS::DSQL::Cluster', 1);
  template.resourceCountIs('AWS::DynamoDB::Table', 3);
  template.resourceCountIs('AWS::S3::Bucket', 1);
  // The Edge stack owns the only bucket policy, so this stack never depends on the distribution.
  template.resourceCountIs('AWS::S3::BucketPolicy', 0);
  template.resourceCountIs('AWS::Lambda::Function', 0);
});

test('retains every stateful resource on delete and replacement', () => {
  for (const type of ['AWS::DSQL::Cluster', 'AWS::DynamoDB::Table', 'AWS::S3::Bucket']) {
    for (const [id, resource] of Object.entries(resources(template, type))) {
      assert.equal(resource.DeletionPolicy, 'Retain', `${id} DeletionPolicy`);
      assert.equal(resource.UpdateReplacePolicy, 'Retain', `${id} UpdateReplacePolicy`);
    }
  }
  template.hasResourceProperties('AWS::DSQL::Cluster', { DeletionProtectionEnabled: true });
});

test('read table is pk/sk, on demand, with PITR', () => {
  template.hasResourceProperties('AWS::DynamoDB::Table', {
    KeySchema: [
      { AttributeName: 'pk', KeyType: 'HASH' },
      { AttributeName: 'sk', KeyType: 'RANGE' },
    ],
    BillingMode: 'PAY_PER_REQUEST',
    PointInTimeRecoverySpecification: { PointInTimeRecoveryEnabled: true },
  });
});

test('sessions table has TTL and the user_id index; limits table has TTL', () => {
  template.hasResourceProperties('AWS::DynamoDB::Table', {
    KeySchema: [{ AttributeName: 'pk', KeyType: 'HASH' }],
    TimeToLiveSpecification: { AttributeName: 'expires', Enabled: true },
    GlobalSecondaryIndexes: [
      Match.objectLike({ IndexName: 'user_id', KeySchema: [{ AttributeName: 'user_id', KeyType: 'HASH' }] }),
    ],
  });
  const ttlTables = Object.values(resources(template, 'AWS::DynamoDB::Table')).filter(
    (t) => t.Properties.TimeToLiveSpecification?.Enabled === true,
  );
  assert.equal(ttlTables.length, 2);
  for (const table of Object.values(resources(template, 'AWS::DynamoDB::Table'))) {
    assert.equal(table.Properties.BillingMode, 'PAY_PER_REQUEST');
  }
});

test('bucket is private, encrypted, versioned and emits EventBridge events', () => {
  template.hasResourceProperties('AWS::S3::Bucket', {
    PublicAccessBlockConfiguration: {
      BlockPublicAcls: true,
      BlockPublicPolicy: true,
      IgnorePublicAcls: true,
      RestrictPublicBuckets: true,
    },
    BucketEncryption: {
      ServerSideEncryptionConfiguration: [{ ServerSideEncryptionByDefault: { SSEAlgorithm: 'AES256' } }],
    },
    VersioningConfiguration: { Status: 'Enabled' },
    NotificationConfiguration: { EventBridgeConfiguration: { EventBridgeEnabled: true } },
  });
});

test('lifecycle expires landing areas after a day and superseded output after 30 days', () => {
  template.hasResourceProperties('AWS::S3::Bucket', {
    LifecycleConfiguration: {
      Rules: [
        Match.objectLike({ Id: 'incoming', Prefix: 'site/incoming/', ExpirationInDays: 1, Status: 'Enabled' }),
        Match.objectLike({ Id: 'imports', Prefix: 'site/imports/', ExpirationInDays: 1, Status: 'Enabled' }),
        Match.objectLike({
          Id: 'superseded',
          NoncurrentVersionExpiration: { NoncurrentDays: 30 },
          ExpiredObjectDeleteMarker: true,
          AbortIncompleteMultipartUpload: { DaysAfterInitiation: 1 },
        }),
      ],
    },
  });
});
