# Ferropress AWS CDK app

Infrastructure as code for the serverless profile (`docs/aws/architecture.md`, sections 3, 4, 6, 10 and 12).
This app only synthesizes CloudFormation templates. Deploying them is a separate step that needs owner
authorization and is not part of this directory's workflow.

## Stacks (one set per site environment)

| Stack | Contents |
|---|---|
| `fp-<site>-<env>-data` | Aurora DSQL cluster, DynamoDB read, sessions and limits tables, and the S3 bucket. Every stateful resource is retained on delete. |
| `fp-<site>-<env>-compute` | The ten functions (arm64, `provided.al2023`), one least-privilege role each, function URLs (`AWS_IAM`), Scheduler schedules, S3 object-created rules (EventBridge), the SQS on-failure queue and the salts secret. |
| `fp-<site>-<env>-edge` | The `FerropressEdge` construct: OAC, the `fp-public` cache policy, response headers policies, CloudFront Functions and the behavior table. It also owns the bucket policy, the CloudFront invoke permissions, the distribution-id SSM parameter and the projector's invalidation grant. |
| `fp-<site>-<env>-ops` | The `Observability` construct: SNS email topic, section 10 alarms, the USD 10 monthly budget and the dashboard. |

The stacks are environment-agnostic: they set no account or region and do no lookups, so synthesis needs no AWS
credentials. CloudFront metrics and certificates live in us-east-1, so deploy the Edge and Ops stacks there.

## Context

| Key | Required | Meaning |
|---|---|---|
| `site` | yes | Short site name used in resource names (`[a-z][a-z0-9-]{0,23}`). |
| `envName` | yes | Environment name such as `dev` or `prod`. |
| `alarmEmail` | yes | Address for alarm and budget notifications. |
| `mailIdentity` | yes | SES identity that `auth` may send from. |
| `basePath` | no | Public base path, `/blog/` by default, or `/` for a whole subdomain. |
| `distributionId`, `distributionDomain` | no | Mount on an existing distribution. The Edge stack then creates the shared pieces and outputs `BehaviorManifest`, which lists the origins and behaviors that the distribution's owner attaches. |
| `domainName`, `certificateArn` | no | Create a subdomain distribution with this alias and its ACM certificate (us-east-1). |
| `assetDir` | no | Directory that holds `lambda-<name>/bootstrap` for every function. The default is `../../../target/lambda`, the cargo-lambda output that CI builds for `aarch64-unknown-linux-musl`. |

With neither distribution option set, the Edge stack creates a distribution on its default `cloudfront.net` host.
`cdk.json` holds example values (`example`, `dev`, `example.com`).

## Synthesize and test

```bash
npm ci
npm test                                          # tsc, then node:test with aws-cdk-lib/assertions
npx cdk synth -c assetDir=test/fixtures/lambda    # or: npm run synth
```

`test/fixtures/lambda/` holds one small placeholder `bootstrap` per function. Tests and the CI synth use it, so
neither needs the Rust build. A real deployment points `assetDir` at the CI build output. Synthesis fails if any
function's directory is missing.

## Contracts for the functions

- Environment variables carry only names and endpoints, never secrets:
  - `FP_SITE`, `FP_BASE_PATH` and `FP_BUCKET`
  - `FP_READ_TABLE`, `FP_SESSIONS_TABLE` and `FP_LIMITS_TABLE`
  - `FP_DSQL_ENDPOINT`
  - `FP_SECRET_ARN`, the ARN of the salts secret, which is read at init
  - `FP_PROJECT_FUNCTION`, used for the async nudge
  - `FP_MAIL_IDENTITY`
  - `FP_DISTRIBUTION_PARAM`, the SSM parameter that names the distribution to invalidate

  Each function gets only the variables for resources its role may touch.
- Session items have partition key `pk`, TTL attribute `expires` and a numeric `user_id`, which the `user_id` index
  covers. Limit items have partition key `pk` and TTL attribute `expires`.
- The projector publishes the EMF metric `Ferropress/OutboxAgeSeconds` with dimensions `Site` and `Env`. The
  outbox-age alarm reads it.
- S3 object-created events arrive through EventBridge (`detail-type: Object Created`), not as S3 notification
  events.
