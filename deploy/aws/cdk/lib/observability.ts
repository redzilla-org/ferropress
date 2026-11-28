// Observability for one site environment (architecture section 10): the SNS email topic, the
// alarms, the monthly budget and the dashboard. Phase 13 extends this construct; keep every
// alarm and widget here so there is one place to look.
import { Duration } from 'aws-cdk-lib';
import * as budgets from 'aws-cdk-lib/aws-budgets';
import * as cloudwatch from 'aws-cdk-lib/aws-cloudwatch';
import * as cwActions from 'aws-cdk-lib/aws-cloudwatch-actions';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as sns from 'aws-cdk-lib/aws-sns';
import * as subscriptions from 'aws-cdk-lib/aws-sns-subscriptions';
import * as sqs from 'aws-cdk-lib/aws-sqs';
import { Construct } from 'constructs';
import { namePrefix, SiteConfig } from './config';

/** Namespace and name of the EMF metric the projector publishes (section 5, lag alarm). */
export const OUTBOX_AGE_METRIC = { namespace: 'Ferropress', metricName: 'OutboxAgeSeconds' };

/** Monthly cost ceiling in USD that triggers the budget notification. */
export const BUDGET_USD = 10;

export interface ObservabilityProps {
  readonly config: SiteConfig;
  readonly readFunction: lambda.IFunction;
  readonly failureQueue: sqs.IQueue;
  readonly distributionId: string;
}

export class Observability extends Construct {
  public readonly topic: sns.Topic;
  public readonly alarms: cloudwatch.Alarm[];
  public readonly dashboard: cloudwatch.Dashboard;

  constructor(scope: Construct, id: string, props: ObservabilityProps) {
    super(scope, id);
    const { config } = props;
    const prefix = namePrefix(config);
    const period = Duration.minutes(5);

    this.topic = new sns.Topic(this, 'AlarmTopic', { displayName: `${prefix} alarms`, enforceSSL: true });
    this.topic.addSubscription(new subscriptions.EmailSubscription(config.alarmEmail));

    // Read Lambda health: error rate as a percentage of invocations, and tail latency.
    const invocations = props.readFunction.metricInvocations({ period, statistic: 'Sum' });
    const errors = props.readFunction.metricErrors({ period, statistic: 'Sum' });
    const readErrorRate = new cloudwatch.MathExpression({
      expression: 'IF(invocations > 0, 100 * errors / invocations, 0)',
      usingMetrics: { invocations, errors },
      period,
      label: 'read error rate (%)',
    });
    const readP99 = props.readFunction.metricDuration({ period, statistic: 'p99' });

    // CloudFront publishes distribution metrics in us-east-1 with Region=Global.
    const cloudFront5xx = new cloudwatch.Metric({
      namespace: 'AWS/CloudFront',
      metricName: '5xxErrorRate',
      dimensionsMap: { DistributionId: props.distributionId, Region: 'Global' },
      statistic: 'Average',
      period,
    });
    const outboxAge = new cloudwatch.Metric({
      ...OUTBOX_AGE_METRIC,
      dimensionsMap: { Site: config.site, Env: config.envName },
      statistic: 'Maximum',
      period,
    });
    const failedAsync = props.failureQueue.metricApproximateNumberOfMessagesVisible({ period, statistic: 'Maximum' });

    // Thresholds from section 10. Missing data (no traffic) is healthy.
    const alarm = (alarmId: string, metric: cloudwatch.IMetric, threshold: number, description: string) =>
      new cloudwatch.Alarm(this, alarmId, {
        alarmName: `${prefix}-${alarmId}`,
        alarmDescription: description,
        metric,
        threshold,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD,
        evaluationPeriods: 1,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      });
    this.alarms = [
      alarm('read-errors', readErrorRate, 1, 'read Lambda error rate above 1%'),
      alarm('read-p99', readP99, 500, 'read Lambda p99 duration above 500 ms'),
      alarm('cloudfront-5xx', cloudFront5xx, 1, 'CloudFront 5xx rate above 1%'),
      alarm('outbox-age', outboxAge, 300, 'oldest unprocessed outbox row older than 5 minutes'),
      alarm('async-failures', failedAsync, 0, 'async invocations landed on the on-failure queue'),
    ];
    const action = new cwActions.SnsAction(this.topic);
    for (const a of this.alarms) {
      a.addAlarmAction(action);
      a.addOkAction(action);
    }

    // Account-level monthly cost budget; notifies the same address once actual spend passes it.
    new budgets.CfnBudget(this, 'Budget', {
      budget: {
        budgetName: `${prefix}-monthly`,
        budgetType: 'COST',
        timeUnit: 'MONTHLY',
        budgetLimit: { amount: BUDGET_USD, unit: 'USD' },
      },
      notificationsWithSubscribers: [
        {
          notification: {
            notificationType: 'ACTUAL',
            comparisonOperator: 'GREATER_THAN',
            threshold: 100,
            thresholdType: 'PERCENTAGE',
          },
          subscribers: [{ subscriptionType: 'EMAIL', address: config.alarmEmail }],
        },
      ],
    });

    // One dashboard per site environment.
    this.dashboard = new cloudwatch.Dashboard(this, 'Dashboard', { dashboardName: prefix });
    this.dashboard.addWidgets(
      new cloudwatch.GraphWidget({ title: 'read invocations and errors', left: [invocations, errors] }),
      new cloudwatch.GraphWidget({ title: 'read error rate (%)', left: [readErrorRate] }),
      new cloudwatch.GraphWidget({ title: 'read p99 (ms)', left: [readP99] }),
    );
    this.dashboard.addWidgets(
      new cloudwatch.GraphWidget({ title: 'CloudFront 5xx (%)', left: [cloudFront5xx] }),
      new cloudwatch.GraphWidget({ title: 'outbox age (s)', left: [outboxAge] }),
      new cloudwatch.GraphWidget({ title: 'failed async invocations', left: [failedAsync] }),
    );
  }
}
