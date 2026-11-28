// The edge construct (architecture section 6): origins, policies and behaviors for one blog,
// mountable on a distribution this app creates or exported for one it does not own.
import { Duration, Fn, Stack } from 'aws-cdk-lib';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import * as origins from 'aws-cdk-lib/aws-cloudfront-origins';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as s3 from 'aws-cdk-lib/aws-s3';
import { Construct } from 'constructs';
import { namePrefix, SiteConfig } from './config';
import { SITE_PREFIX } from './data-stack';
import { routerCode, uploadsRewriteCode } from './edge-functions';
import { URL_FUNCTIONS, UrlFunctionName } from './functions';

/** Query strings that select different public output; everything else is ignored by the cache. */
export const PUBLIC_QUERY_ALLOWLIST = ['p', 'page_id', 'paged', 'cat', 'tag', 'author', 'm', 'feed', 's'];

export type BehaviorKind = 'media' | 'dynamic' | 'public' | 'bare';

/** One row of the section 6 behavior table. */
export interface BehaviorSpec {
  readonly pattern: string;
  readonly origin: 'uploads' | UrlFunctionName;
  readonly kind: BehaviorKind;
  /** Admin-only response headers (frame-ancestors 'self'). */
  readonly admin: boolean;
}

/**
 * The behavior table for a base path, in precedence order. For base path `/` the public row
 * is the distribution's default behavior and there is no bare-path row.
 */
export function behaviorTable(basePath: string): BehaviorSpec[] {
  const b = basePath.slice(1);
  const rows: BehaviorSpec[] = [
    { pattern: `${b}wp-content/uploads/*`, origin: 'uploads', kind: 'media', admin: false },
    // The media signer sits under api/ and must win over the admin catch-all.
    { pattern: `${b}api/media/sign`, origin: 'media-sign', kind: 'dynamic', admin: true },
    { pattern: `${b}admin/*`, origin: 'admin', kind: 'dynamic', admin: true },
    { pattern: `${b}api/*`, origin: 'admin', kind: 'dynamic', admin: true },
    { pattern: `${b}login*`, origin: 'auth', kind: 'dynamic', admin: true },
    { pattern: `${b}comments*`, origin: 'comments', kind: 'dynamic', admin: false },
    { pattern: `${b}*`, origin: 'read', kind: 'public', admin: false },
  ];
  if (b !== '') {
    rows.push({ pattern: b.slice(0, -1), origin: 'read', kind: 'bare', admin: false });
  }
  return rows;
}

export interface FerropressEdgeProps {
  readonly config: SiteConfig;
  readonly bucket: s3.IBucket;
  readonly urls: Record<UrlFunctionName, lambda.IFunctionUrl>;
}

export class FerropressEdge extends Construct {
  public readonly publicCachePolicy: cloudfront.CachePolicy;
  public readonly publicHeaders: cloudfront.ResponseHeadersPolicy;
  public readonly adminHeaders: cloudfront.ResponseHeadersPolicy;
  public readonly lambdaOac: cloudfront.FunctionUrlOriginAccessControl;
  public readonly s3Oac: cloudfront.S3OriginAccessControl;
  public readonly router: cloudfront.Function;
  public readonly uploadsRewrite: cloudfront.Function;
  /** Function URL host names, which CloudFront uses as origin domains. */
  public readonly originDomains: Record<UrlFunctionName, string>;
  private readonly props: FerropressEdgeProps;
  private readonly origins: Record<BehaviorSpec['origin'], cloudfront.IOrigin>;

  constructor(scope: Construct, id: string, props: FerropressEdgeProps) {
    super(scope, id);
    this.props = props;
    const prefix = namePrefix(props.config);

    // fp-public: path plus the allowlisted query strings, no cookies, compressed variants.
    // Long TTLs because freshness comes from invalidation, not expiry.
    this.publicCachePolicy = new cloudfront.CachePolicy(this, 'PublicCachePolicy', {
      cachePolicyName: `fp-public-${props.config.site}-${props.config.envName}`,
      comment: 'Ferropress public pages, feeds and sitemaps',
      queryStringBehavior: cloudfront.CacheQueryStringBehavior.allowList(...PUBLIC_QUERY_ALLOWLIST),
      headerBehavior: cloudfront.CacheHeaderBehavior.none(),
      cookieBehavior: cloudfront.CacheCookieBehavior.none(),
      enableAcceptEncodingGzip: true,
      enableAcceptEncodingBrotli: true,
      minTtl: Duration.seconds(0),
      defaultTtl: Duration.days(1),
      maxTtl: Duration.days(365),
    });

    // Security headers on every response; admin responses also refuse foreign framing.
    const securityHeaders: cloudfront.ResponseSecurityHeadersBehavior = {
      strictTransportSecurity: {
        accessControlMaxAge: Duration.days(730),
        includeSubdomains: true,
        override: true,
      },
      contentTypeOptions: { override: true },
      referrerPolicy: {
        referrerPolicy: cloudfront.HeadersReferrerPolicy.STRICT_ORIGIN_WHEN_CROSS_ORIGIN,
        override: true,
      },
    };
    this.publicHeaders = new cloudfront.ResponseHeadersPolicy(this, 'PublicHeaders', {
      responseHeadersPolicyName: `${prefix}-public-headers`,
      securityHeadersBehavior: securityHeaders,
    });
    this.adminHeaders = new cloudfront.ResponseHeadersPolicy(this, 'AdminHeaders', {
      responseHeadersPolicyName: `${prefix}-admin-headers`,
      securityHeadersBehavior: {
        ...securityHeaders,
        contentSecurityPolicy: { contentSecurityPolicy: "frame-ancestors 'self'", override: true },
      },
    });

    // OAC signs origin requests with SigV4, so the IAM-authenticated URLs and the private
    // bucket accept only this distribution.
    this.lambdaOac = new cloudfront.FunctionUrlOriginAccessControl(this, 'LambdaOac', {
      originAccessControlName: `${prefix}-lambda`,
      signing: cloudfront.Signing.SIGV4_ALWAYS,
    });
    this.s3Oac = new cloudfront.S3OriginAccessControl(this, 'S3Oac', {
      originAccessControlName: `${prefix}-s3`,
      signing: cloudfront.Signing.SIGV4_ALWAYS,
    });

    // A function URL is https://<host>/; CloudFront wants the bare host.
    const domains = {} as Record<UrlFunctionName, string>;
    for (const name of URL_FUNCTIONS) {
      domains[name] = Fn.select(2, Fn.split('/', props.urls[name].url));
    }
    this.originDomains = domains;

    this.router = new cloudfront.Function(this, 'Router', {
      functionName: `${prefix}-router`,
      comment: 'Ferropress: bare base path redirect and search routing',
      runtime: cloudfront.FunctionRuntime.JS_2_0,
      code: cloudfront.FunctionCode.fromInline(routerCode(props.config.basePath, domains.search)),
    });
    this.uploadsRewrite = new cloudfront.Function(this, 'UploadsRewrite', {
      functionName: `${prefix}-uploads`,
      comment: 'Ferropress: map the public uploads path onto the bucket layout',
      runtime: cloudfront.FunctionRuntime.JS_2_0,
      code: cloudfront.FunctionCode.fromInline(uploadsRewriteCode(props.config.basePath)),
    });

    // Origins: HTTPS-only function URLs and the bucket's site/ prefix, all behind OAC.
    const fnOrigin = (name: UrlFunctionName) =>
      new origins.HttpOrigin(domains[name], {
        protocolPolicy: cloudfront.OriginProtocolPolicy.HTTPS_ONLY,
        originAccessControlId: this.lambdaOac.originAccessControlId,
        originId: `${prefix}-${name}`,
      });
    const originMap = {
      uploads: origins.S3BucketOrigin.withBucketDefaults(props.bucket, {
        originAccessControlId: this.s3Oac.originAccessControlId,
        originPath: `/${SITE_PREFIX.slice(0, -1)}`,
        originId: `${prefix}-uploads`,
      }),
    } as Record<BehaviorSpec['origin'], cloudfront.IOrigin>;
    for (const name of URL_FUNCTIONS) {
      originMap[name] = fnOrigin(name);
    }
    this.origins = originMap;
  }

  /** CloudFront behavior options for one row of the table. */
  public behaviorOptions(spec: BehaviorSpec): cloudfront.BehaviorOptions {
    const viewerRequest = (fn: cloudfront.IFunction) => [
      { function: fn, eventType: cloudfront.FunctionEventType.VIEWER_REQUEST },
    ];
    const common = {
      origin: this.origins[spec.origin],
      viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
      compress: true,
      responseHeadersPolicy: spec.admin ? this.adminHeaders : this.publicHeaders,
    };
    // Function URLs need their own Host, hence AllViewerExceptHostHeader on every Lambda row.
    const toLambda = cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER;
    switch (spec.kind) {
      case 'media':
        return {
          ...common,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_GET_HEAD,
          cachePolicy: cloudfront.CachePolicy.CACHING_OPTIMIZED,
          functionAssociations: viewerRequest(this.uploadsRewrite),
        };
      case 'dynamic':
        return {
          ...common,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: toLambda,
        };
      case 'public':
        return {
          ...common,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_GET_HEAD,
          cachePolicy: this.publicCachePolicy,
          originRequestPolicy: toLambda,
          functionAssociations: viewerRequest(this.router),
        };
      case 'bare':
        return {
          ...common,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_GET_HEAD,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: toLambda,
          functionAssociations: viewerRequest(this.router),
        };
    }
  }

  /** The behavior that serves the public pages; the default behavior of a created distribution. */
  public publicBehavior(): cloudfront.BehaviorOptions {
    return this.behaviorOptions(this.rows().find((r) => r.kind === 'public') as BehaviorSpec);
  }

  /** Behavior rows in precedence order; `mount` skips a `*` row, which is the default behavior. */
  public rows(): BehaviorSpec[] {
    return behaviorTable(this.props.config.basePath);
  }

  /** Adds every behavior to a distribution this app manages, at the configured base path. */
  public mount(distribution: cloudfront.Distribution): void {
    for (const spec of this.rows()) {
      if (spec.pattern !== '*') {
        distribution.addBehavior(spec.pattern, this.origins[spec.origin], this.withoutOrigin(spec));
      }
    }
  }

  /**
   * Lets one distribution invoke the function URLs and read media. The bucket policy is owned
   * here (not by the Data stack) so the Data stack never depends on the distribution.
   */
  public allowDistribution(distributionArn: string, functions: Record<UrlFunctionName, lambda.IFunction>): void {
    const principal = 'cloudfront.amazonaws.com';
    for (const name of URL_FUNCTIONS) {
      const id = name.replace(/-(.)/g, (_, c: string) => c.toUpperCase());
      new lambda.CfnPermission(this, `${id}InvokeUrl`, {
        action: 'lambda:InvokeFunctionUrl',
        functionName: functions[name].functionArn,
        functionUrlAuthType: 'AWS_IAM',
        principal,
        sourceArn: distributionArn,
      });
      new lambda.CfnPermission(this, `${id}Invoke`, {
        action: 'lambda:InvokeFunction',
        functionName: functions[name].functionArn,
        invokedViaFunctionUrl: true,
        principal,
        sourceArn: distributionArn,
      });
    }

    const bucket = this.props.bucket;
    const document = new iam.PolicyDocument({
      statements: [
        new iam.PolicyStatement({
          sid: 'TlsOnly',
          effect: iam.Effect.DENY,
          principals: [new iam.AnyPrincipal()],
          actions: ['s3:*'],
          resources: [bucket.bucketArn, bucket.arnForObjects('*')],
          conditions: { Bool: { 'aws:SecureTransport': 'false' } },
        }),
        new iam.PolicyStatement({
          sid: 'CloudFrontReadsMedia',
          principals: [new iam.ServicePrincipal(principal)],
          actions: ['s3:GetObject'],
          resources: [bucket.arnForObjects(`${SITE_PREFIX}uploads/*`)],
          conditions: { StringEquals: { 'AWS:SourceArn': distributionArn } },
        }),
      ],
    });
    new s3.CfnBucketPolicy(this, 'BucketPolicy', { bucket: bucket.bucketName, policyDocument: document });
  }

  /** The behavior table with resolved policy ids, for mounting on a distribution owned elsewhere. */
  public manifest(): string {
    return Stack.of(this).toJsonString(
      this.rows().map((spec) => {
        const options = this.behaviorOptions(spec);
        return {
          pathPattern: spec.pattern,
          originId: `${namePrefix(this.props.config)}-${spec.origin}`,
          originDomain: spec.origin === 'uploads' ? this.props.bucket.bucketRegionalDomainName : this.originDomains[spec.origin],
          originPath: spec.origin === 'uploads' ? `/${SITE_PREFIX.slice(0, -1)}` : '',
          originAccessControlId: spec.origin === 'uploads' ? this.s3Oac.originAccessControlId : this.lambdaOac.originAccessControlId,
          allowedMethods: options.allowedMethods?.methods,
          cachePolicyId: options.cachePolicy?.cachePolicyRef.cachePolicyId,
          originRequestPolicyId: options.originRequestPolicy?.originRequestPolicyRef.originRequestPolicyId,
          responseHeadersPolicyId: options.responseHeadersPolicy?.responseHeadersPolicyRef.responseHeadersPolicyId,
          viewerRequestFunctionArn: options.functionAssociations?.[0]?.function.functionRef.functionArn,
        };
      }),
    );
  }

  private withoutOrigin(spec: BehaviorSpec): cloudfront.AddBehaviorOptions {
    const { origin: _origin, ...rest } = this.behaviorOptions(spec);
    return rest;
  }
}
