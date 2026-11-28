// The Lambda split of architecture section 3 as data, so the Compute stack and the tests read
// the same table.

export type FunctionName =
  | 'read'
  | 'admin'
  | 'auth'
  | 'media-sign'
  | 'media-process'
  | 'search'
  | 'comments'
  | 'project'
  | 'cron'
  | 'import';

export interface FunctionSpec {
  readonly name: FunctionName;
  readonly memoryMb: number;
  readonly timeoutSeconds: number;
  /** Reached by CloudFront through a function URL with AuthType AWS_IAM. */
  readonly url: boolean;
  /** Invoked asynchronously (schedule, S3 event, nudge), so failures go to the SQS queue. */
  readonly async: boolean;
}

export const FUNCTIONS: readonly FunctionSpec[] = [
  { name: 'read', memoryMb: 128, timeoutSeconds: 3, url: true, async: false },
  { name: 'admin', memoryMb: 1024, timeoutSeconds: 29, url: true, async: false },
  { name: 'auth', memoryMb: 256, timeoutSeconds: 10, url: true, async: false },
  { name: 'media-sign', memoryMb: 128, timeoutSeconds: 5, url: true, async: false },
  { name: 'media-process', memoryMb: 2048, timeoutSeconds: 120, url: false, async: true },
  { name: 'search', memoryMb: 256, timeoutSeconds: 5, url: true, async: false },
  { name: 'comments', memoryMb: 256, timeoutSeconds: 10, url: true, async: false },
  { name: 'project', memoryMb: 1024, timeoutSeconds: 300, url: false, async: true },
  { name: 'cron', memoryMb: 512, timeoutSeconds: 300, url: false, async: true },
  { name: 'import', memoryMb: 2048, timeoutSeconds: 900, url: false, async: true },
];

/** Cargo binary (and cargo-lambda output directory) for a function: `lambda-<name>`. */
export function binaryName(name: FunctionName): string {
  return `lambda-${name}`;
}

/** Functions with a URL, in a fixed order. */
export type UrlFunctionName = 'read' | 'admin' | 'auth' | 'media-sign' | 'search' | 'comments';

export const URL_FUNCTIONS: readonly UrlFunctionName[] = FUNCTIONS.filter((f) => f.url).map(
  (f) => f.name as UrlFunctionName,
);
