// Ops stack: the section 10 alarms, email topic, budget and dashboard.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Template } from 'aws-cdk-lib/assertions';
import { BUDGET_USD } from '../lib/observability';
import { flat, resources, site } from './helpers';

const template = Template.fromStack(site().ops);

test('alarms notify one email topic', () => {
  template.resourceCountIs('AWS::SNS::Topic', 1);
  template.hasResourceProperties('AWS::SNS::Subscription', { Protocol: 'email', Endpoint: 'alerts@example.com' });
  const alarms = Object.values(resources(template, 'AWS::CloudWatch::Alarm'));
  assert.equal(alarms.length, 5);
  for (const alarm of alarms) {
    assert.match(flat(alarm.Properties.AlarmActions), /AlarmTopic/);
    assert.equal(alarm.Properties.ComparisonOperator, 'GreaterThanThreshold');
    assert.equal(alarm.Properties.TreatMissingData, 'notBreaching');
  }
});

test('thresholds follow section 10', () => {
  const byName = Object.fromEntries(
    Object.values(resources(template, 'AWS::CloudWatch::Alarm')).map((a) => [a.Properties.AlarmName, a.Properties]),
  );
  assert.equal(byName['fp-example-dev-read-errors'].Threshold, 1);
  assert.match(flat(byName['fp-example-dev-read-errors'].Metrics), /100 \* errors \/ invocations/);
  assert.equal(byName['fp-example-dev-read-p99'].Threshold, 500);
  assert.equal(byName['fp-example-dev-read-p99'].ExtendedStatistic, 'p99');
  assert.equal(byName['fp-example-dev-cloudfront-5xx'].Threshold, 1);
  assert.equal(byName['fp-example-dev-cloudfront-5xx'].MetricName, '5xxErrorRate');
  assert.equal(byName['fp-example-dev-outbox-age'].Threshold, 300);
  assert.equal(byName['fp-example-dev-outbox-age'].Namespace, 'Ferropress');
  assert.equal(byName['fp-example-dev-async-failures'].Threshold, 0);
  assert.match(flat(byName['fp-example-dev-async-failures'].Dimensions), /FailureQueue/);
});

test('a USD 10 monthly budget emails the same address', () => {
  assert.equal(BUDGET_USD, 10);
  template.hasResourceProperties('AWS::Budgets::Budget', {
    Budget: { BudgetName: 'fp-example-dev-monthly', BudgetType: 'COST', TimeUnit: 'MONTHLY', BudgetLimit: { Amount: 10, Unit: 'USD' } },
    NotificationsWithSubscribers: [
      {
        Notification: { NotificationType: 'ACTUAL', ComparisonOperator: 'GREATER_THAN', Threshold: 100, ThresholdType: 'PERCENTAGE' },
        Subscribers: [{ SubscriptionType: 'EMAIL', Address: 'alerts@example.com' }],
      },
    ],
  });
});

test('one dashboard per site environment', () => {
  template.resourceCountIs('AWS::CloudWatch::Dashboard', 1);
  template.hasResourceProperties('AWS::CloudWatch::Dashboard', { DashboardName: 'fp-example-dev' });
});
