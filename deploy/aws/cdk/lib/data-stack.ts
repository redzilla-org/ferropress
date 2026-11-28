// Data stack: the stateful resources of one site environment (architecture section 4).
// Everything here is retained on stack delete or replacement, so a mistaken teardown never
// destroys posts, sessions or media.
import { Duration, RemovalPolicy, Stack, StackProps, Tags } from 'aws-cdk-lib';
import * as dsql from 'aws-cdk-lib/aws-dsql';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as s3 from 'aws-cdk-lib/aws-s3';
import { Construct } from 'constructs';
import { SiteConfig } from './config';

/** Key prefix under which every object of the site lives in the bucket. */
export const SITE_PREFIX = 'site/';

export interface DataStackProps extends StackProps {
  readonly config: SiteConfig;
}

export class DataStack extends Stack {
  public readonly cluster: dsql.CfnCluster;
  public readonly readTable: dynamodb.Table;
  public readonly sessionsTable: dynamodb.Table;
  public readonly limitsTable: dynamodb.Table;
  public readonly bucket: s3.Bucket;

  constructor(scope: Construct, id: string, props: DataStackProps) {
    super(scope, id, props);
    Tags.of(this).add('ferropress:site', props.config.site);
    Tags.of(this).add('ferropress:env', props.config.envName);

    // DSQL is the relational source of truth; deletion protection guards it at the API level
    // in addition to the CloudFormation retain policy.
    this.cluster = new dsql.CfnCluster(this, 'Cluster', { deletionProtectionEnabled: true });
    this.cluster.applyRemovalPolicy(RemovalPolicy.RETAIN);

    // The hot read model: route, post doc, term listing and checkpoint items share pk/sk.
    // On-demand fits a blog's spiky, tiny traffic; PITR covers projector bugs.
    this.readTable = new dynamodb.Table(this, 'ReadTable', {
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'sk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
      removalPolicy: RemovalPolicy.RETAIN,
    });

    // Sessions expire by TTL; the user_id index serves "log out everywhere" (section 7).
    this.sessionsTable = new dynamodb.Table(this, 'SessionsTable', {
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'expires',
      removalPolicy: RemovalPolicy.RETAIN,
    });
    this.sessionsTable.addGlobalSecondaryIndex({
      indexName: 'user_id',
      partitionKey: { name: 'user_id', type: dynamodb.AttributeType.NUMBER },
      projectionType: dynamodb.ProjectionType.KEYS_ONLY,
    });

    // Rate-limit counters and transients: short-lived items that expire by TTL.
    this.limitsTable = new dynamodb.Table(this, 'LimitsTable', {
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'expires',
      removalPolicy: RemovalPolicy.RETAIN,
    });

    // Media and pre-rendered output. Private: only CloudFront (OAC, granted by the Edge stack,
    // which owns the bucket policy) and the function roles read it.
    this.bucket = new s3.Bucket(this, 'Bucket', {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
      versioned: true,
      removalPolicy: RemovalPolicy.RETAIN,
      lifecycleRules: [
        // Upload landing areas are consumed within minutes; leftovers go after a day.
        { id: 'incoming', prefix: `${SITE_PREFIX}incoming/`, expiration: Duration.days(1) },
        { id: 'imports', prefix: `${SITE_PREFIX}imports/`, expiration: Duration.days(1) },
        // Content-addressed output is never overwritten: the projector deletes unreferenced
        // objects, and their noncurrent versions stay 30 days for rollback.
        {
          id: 'superseded',
          noncurrentVersionExpiration: Duration.days(30),
          expiredObjectDeleteMarker: true,
          abortIncompleteMultipartUploadAfter: Duration.days(1),
        },
      ],
    });
    // Native EventBridge delivery for object events: no notification custom resource, and the
    // Compute stack subscribes without a dependency back onto this stack.
    const cfnBucket = this.bucket.node.defaultChild as s3.CfnBucket;
    cfnBucket.addPropertyOverride('NotificationConfiguration.EventBridgeConfiguration', {
      EventBridgeEnabled: true,
    });
  }
}
