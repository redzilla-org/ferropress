// Ops stack: holds the Observability construct. It comes last because its alarms read the
// read function and failure queue (Compute) and the distribution id (Edge).
import { Stack, StackProps, Tags } from 'aws-cdk-lib';
import { Construct } from 'constructs';
import { SiteConfig } from './config';
import { ComputeStack } from './compute-stack';
import { EdgeStack } from './edge-stack';
import { Observability } from './observability';

export interface OpsStackProps extends StackProps {
  readonly config: SiteConfig;
  readonly compute: ComputeStack;
  readonly edge: EdgeStack;
}

export class OpsStack extends Stack {
  public readonly observability: Observability;

  constructor(scope: Construct, id: string, props: OpsStackProps) {
    super(scope, id, props);
    Tags.of(this).add('ferropress:site', props.config.site);
    Tags.of(this).add('ferropress:env', props.config.envName);
    this.observability = new Observability(this, 'Observability', {
      config: props.config,
      readFunction: props.compute.functions.read,
      failureQueue: props.compute.failureQueue,
      distributionId: props.edge.distributionId,
    });
  }
}
