// Edge stack: CloudFront in front of the function URLs and the bucket. It either creates a
// distribution (a subdomain, or the default cloudfront.net host) or, given an existing
// distribution id, creates the shared pieces and exports the behavior manifest to mount there.
import { CfnOutput, Stack, StackProps, Tags } from 'aws-cdk-lib';
import * as acm from 'aws-cdk-lib/aws-certificatemanager';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as ssm from 'aws-cdk-lib/aws-ssm';
import { Construct } from 'constructs';
import { SiteConfig } from './config';
import { ComputeStack, distributionParameterName } from './compute-stack';
import { DataStack } from './data-stack';
import { FerropressEdge } from './edge';

export interface EdgeStackProps extends StackProps {
  readonly config: SiteConfig;
  readonly data: DataStack;
  readonly compute: ComputeStack;
}

export class EdgeStack extends Stack {
  public readonly edge: FerropressEdge;
  public readonly distributionId: string;
  /** Set only when this stack created the distribution. */
  public readonly distribution?: cloudfront.Distribution;

  constructor(scope: Construct, id: string, props: EdgeStackProps) {
    super(scope, id, props);
    const { config, data, compute } = props;
    Tags.of(this).add('ferropress:site', config.site);
    Tags.of(this).add('ferropress:env', config.envName);

    this.edge = new FerropressEdge(this, 'Edge', { config, bucket: data.bucket, urls: compute.urls });

    if (config.existingDistribution !== undefined) {
      // A distribution owned elsewhere cannot be edited from here; its owner attaches these
      // origins and behaviors (phase 14 runbook) using the manifest below.
      this.distributionId = config.existingDistribution.id;
      new CfnOutput(this, 'BehaviorManifest', { value: this.edge.manifest() });
    } else {
      const subdomain = config.subdomain;
      this.distribution = new cloudfront.Distribution(this, 'Distribution', {
        comment: `Ferropress ${config.site} ${config.envName}`,
        defaultBehavior: this.edge.publicBehavior(),
        domainNames: subdomain === undefined ? undefined : [subdomain.domainName],
        certificate:
          subdomain === undefined
            ? undefined
            : acm.Certificate.fromCertificateArn(this, 'Certificate', subdomain.certificateArn),
        // The default cloudfront.net certificate has a fixed policy; a custom one gets TLS 1.2+.
        minimumProtocolVersion:
          subdomain === undefined ? undefined : cloudfront.SecurityPolicyProtocol.TLS_V1_2_2021,
        httpVersion: cloudfront.HttpVersion.HTTP2_AND_3,
        priceClass: cloudfront.PriceClass.PRICE_CLASS_100,
      });
      this.edge.mount(this.distribution);
      this.distributionId = this.distribution.distributionId;
      new CfnOutput(this, 'DistributionDomain', { value: this.distribution.distributionDomainName });
    }
    new CfnOutput(this, 'DistributionId', { value: this.distributionId });

    const distributionArn = this.formatArn({
      service: 'cloudfront',
      region: '',
      resource: 'distribution',
      resourceName: this.distributionId,
    });
    this.edge.allowDistribution(distributionArn, compute.functions);

    // The projector learns the distribution through SSM and may invalidate only it. Both live
    // here because the distribution id is known only after this stack, which depends on Compute.
    new ssm.StringParameter(this, 'DistributionIdParameter', {
      parameterName: distributionParameterName(config),
      stringValue: this.distributionId,
      description: 'Ferropress distribution the projector invalidates',
    });
    const projectRole = compute.functions.project.role as iam.IRole;
    new iam.Policy(this, 'ProjectInvalidation', {
      roles: [projectRole],
      statements: [
        new iam.PolicyStatement({ actions: ['cloudfront:CreateInvalidation'], resources: [distributionArn] }),
      ],
    });
  }
}
