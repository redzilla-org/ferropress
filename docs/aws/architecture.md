# Ferropress on AWS Lambda: architecture

Status: proposal, 2026-11-27. Nothing is deployed; no AWS resource exists. Tracking issue: #1.

## 1. What is being ported

Read from this repository at the `lambda-port` branch point:

- **Language and shape.** A Rust 2024 workspace (`Cargo.toml`): a clean-room PHP engine
  (`php-lexer` → `php-parser` → `php-compiler` → `php-vm`, `php-value`, `php-serialize`), a bridge
  (`php-abi-bridge`, `php-bindings`) and native CMS domain crates (`cms-hooks`, `cms-request`,
  `cms-bootstrap`, `cms-options`, `cms-cache`, `cms-content`, `cms-taxonomy`, `cms-comments`,
  `cms-users`, `cms-auth`, `cms-query`). Layering is enforced by `xtask check-layers`
  (`xtask/src/layers.rs`): binaries → platform/infra/bindings → orchestration → domain → foundation.
- **Request path today.** There is no HTTP server yet. `platform-server`, rewrite, formatting,
  themes, blocks and media are roadmap step 24 (not started); admin and REST are Phase 7
  (`docs/ROADMAP.md`). What exists is the data and query layer: `cms-query` parses query vars
  (`p`, `page_id`, `name`, `pagename`, attachment, date, search, author) and loads the singular row.
- **Data model.** The reference schema, unchanged (ADR-0004): posts (revisions and attachments
  are post rows), postmeta, terms, term_taxonomy, term_relationships, users, usermeta, options,
  comments, commentmeta. Cells are raw bytes; domain crates parse rows by column name
  (`crates/cms-content/src/post.rs`).
- **Data access.** One port, `cms_db::Store::query(&Sql) -> Reply` (`crates/cms-db/src/store.rs`):
  MySQL-dialect statement text in, text cells / affected rows / insert id / server error out.
  Statement builders live in each crate's `sql.rs` and emit backtick identifiers and
  backslash-escaped literals, byte-exact to the reference because plugins can observe SQL text.
  The only adapter is `infra-mysql`.
- **Sessions and auth.** `cms-auth` loads the current user by id and checks capabilities from
  usermeta and the `user_roles` option. Login, cookies, nonces and sessions are not implemented.
- **Media.** Attachment query vars only; uploads and image processing are not implemented.
- **Admin vs public.** ADR-0006: Ferropress ships its own admin application hosting the plugin admin
  extension surface; block-editor hosting is undecided. Public routes are theme-rendered pages,
  feeds, sitemaps and REST reads.

Consequence: the Lambda port cannot lift and shift an existing server. It defines the
serverless *deployment profile* that the server, renderer and admin plug into as the roadmap
delivers them. Its first deliverables are a storage-neutral data layer and a tiny read path that
serves pre-rendered output.

## 2. Traffic shape

There is no production Ferropress traffic to measure. The table is an **estimate** for a typical
single-author blog of a few hundred posts mostly reached from search. It will be replaced with
numbers measured from CloudFront standard logs in the first two weeks of the first deployment
(Phase 13 dashboards).

| Route class | Example | Share of HTML/XML requests (estimate) | Served by |
|---|---|---|---|
| Single post | `/blog/2026/11/some-post/` | 60–75% | read Lambda |
| Home and paged home | `/blog/`, `/blog/page/2/` | 8–12% | read Lambda |
| Feeds | `/blog/feed/`, per-term feeds | 5–10% (bots, readers) | read Lambda |
| Term / date / author archives | `/blog/category/x/` | 5–10% | read Lambda |
| Sitemaps, robots | `/blog/wp-sitemap.xml` | 1–3% (crawlers) | read Lambda |
| Pages | `/blog/about/` | 2–5% | read Lambda |
| Search | `/blog/?s=` | under 1% | search Lambda |
| Comments / forms POST | | under 0.5% | comments Lambda |
| Admin, login, REST writes | | under 0.5% | admin / auth Lambdas |

Static assets and uploads (images, CSS, JS) are the majority of all requests by count, but they
are served from S3 through CloudFront and never reach a Lambda. With CloudFront in front, the
read Lambda sees only cache misses. At about 98% of dynamic traffic on cacheable GETs, the read
Lambda is the function worth making tiny.

## 3. Lambda split

All functions: Rust, `provided.al2023`, **arm64**, no VPC (DSQL, DynamoDB and S3 are reached on
their public endpoints with IAM, so there is no NAT and no ENI cold start). Each is a separate
binary under `apps/`, so the read path links none of the PHP engine.

| Function | Purpose | Memory | Timeout | Trigger | IAM scope (resource-level) |
|---|---|---|---|---|---|
| `read` (tiny) | Published posts, pages, home, archives, feeds, sitemaps, 404s, redirects | 128 MB | 3 s | Function URL (AuthType `AWS_IAM`), CloudFront OAC, GET/HEAD | `dynamodb:GetItem` on the read table; `s3:GetObject` on `site/rendered/*`, `site/fragments/*` |
| `admin` | Admin application, plugin admin pages, REST writes, previews | 1024 MB | 29 s | Function URL + OAC, `/blog/admin/*`, `/blog/api/*` | `dsql:DbConnect` (non-admin role); `dynamodb:GetItem` on sessions; `lambda:InvokeFunction` (async) on `project`; `secretsmanager:GetSecretValue` on the site secret |
| `auth` | Login, logout, password reset, 2FA, nonce issue | 256 MB | 10 s | Function URL + OAC, `/blog/login*` | `dsql:DbConnect`; `dynamodb:PutItem/DeleteItem/GetItem` on sessions; `dynamodb:UpdateItem` on rate limits; `ses:SendEmail` on one identity; site secret |
| `media-sign` | Presigned upload URL for the editor | 128 MB | 5 s | Function URL + OAC, `/blog/api/media/sign` | `s3:PutObject` on `site/incoming/*` (presign); sessions read |
| `media-process` | Resize/convert images, write attachment rows | 2048 MB | 120 s | S3 `ObjectCreated` on `site/incoming/` | `s3:GetObject/DeleteObject` `incoming/*`, `s3:PutObject` `uploads/*`; `dsql:DbConnect` |
| `search` | `?s=` pages and REST search | 256 MB | 5 s | Function URL + OAC, CloudFront behavior on query `s` | `s3:GetObject` on `site/search/*` (prebuilt index) |
| `comments` | Comment and form POSTs, moderation queue | 256 MB | 10 s | Function URL + OAC, `/blog/comments*` | `dsql:DbConnect`; rate-limit `UpdateItem`; `lambda:InvokeFunction` on `project` |
| `project` (job) | Drains the outbox: renders affected routes, writes S3 then DynamoDB, invalidates CloudFront, rebuilds feeds, sitemaps and the search index | 1024 MB | 300 s | Async invoke "nudge" after each commit + EventBridge Scheduler every 5 min; reserved concurrency 1 | `dsql:DbConnect`; `s3:PutObject` on `rendered/*`, `fragments/*`, `search/*`; `dynamodb:PutItem/DeleteItem/Query` on the read table; `cloudfront:CreateInvalidation` on one distribution |
| `cron` (job) | Scheduled publishing and the reference's scheduled events | 512 MB | 300 s | EventBridge Scheduler, 1 min | `dsql:DbConnect`; invoke `project` |
| `import` (job) | WXR import (section 9) | 2048 MB | 900 s | S3 `ObjectCreated` on `site/imports/` | `s3:GetObject` `imports/*`, `s3:PutObject` `uploads/*`; `dsql:DbConnect`; checkpoint item in the read table |

### The tiny read Lambda

- **Dependencies.** No async runtime and no AWS SDK. It runs a blocking loop on the Lambda Runtime
  API (`next` → handle → `response`), uses `serde_json` for the event, and a small SigV4 signer
  over `sha2` for one DynamoDB `GetItem` plus at most two S3 `GetObject`s per miss. The binary
  target is under 3 MB stripped and under 20 ms init, with no PHP engine linked.
- **Algorithm per request.** Normalize the path under the base path (`/blog/`) and resolve
  it to one route item in DynamoDB, `pk = ROUTE#<path>`. The item names:
  - its kind: `page`, `object` (feed, sitemap, robots), `redirect` or `gone`;
  - its status, content type, ETag and Last-Modified;
  - its S3 keys.

  For a page, the body fragment and the shell fragment (head, header, footer, sidebar) are separate
  S3 objects. The shell is keyed by its content hash and cached in memory across warm invocations,
  so a theme or menu change re-renders one shell instead of every page. The Lambda answers 304
  for a matching `If-None-Match`. An unknown path returns the pre-rendered 404 page.
- **No state is written** and nothing request-local survives the invocation except the
  immutable, hash-keyed fragment cache (ADR-0005's shared-immutable rule).

## 4. Storage

### Aurora DSQL: relational source of truth

The reference tables (`fp_posts`, `fp_postmeta`, `fp_terms`, `fp_term_taxonomy`,
`fp_term_relationships`, `fp_users`, `fp_usermeta`, `fp_options`, `fp_comments`, `fp_commentmeta`),
plus `fp_outbox` and `fp_schema_migrations`. DDL: `deploy/aws/dsql/0001-core.sql`.

Schema decisions forced by DSQL (AWS docs read 2026-11-27: quotas page `CHAP_quotas.html`,
PostgreSQL-compatibility page, data types page, `CREATE SEQUENCE` page):

- **Foreign keys are supported now, and not used**, because the reference schema has none:
  plugins delete parent rows and clean up children themselves. Adding FKs would change
  observable behavior.
- **IDs:** `bigint GENERATED BY DEFAULT AS IDENTITY (CACHE 1)`. DSQL accepts only `CACHE 1` or
  `>= 65536`. `CACHE 1` keeps IDs sequential like auto-increment, which the reference's id
  tiebreak ordering expects. It costs coordination per insert, which is irrelevant at blog write
  rates. `BY DEFAULT` lets the WXR import keep source IDs.
- **Optimistic concurrency.** Isolation is fixed at Repeatable Read and conflicts fail at commit
  (SQLSTATE `40001`, DSQL `OC000`/`OC001`). Every write path runs as a closure retried with
  jittered backoff. The reference's hot single rows (the `cron` option, transients in
  `fp_options`) are kept out of DSQL: transients and the object cache's persistent layer go to
  DynamoDB, and scheduled events are owned by the `cron` Lambda's schedule table.
- **Transaction limits.** 3,000 modified rows, 10 MiB and 5 minutes per transaction. Bulk paths
  (import, empty trash, term re-count, delete user with content) are chunked at 2,500 rows or
  8 MiB, whichever comes first.
- **DDL.** One DDL statement per transaction, DDL never mixed with DML, `CREATE INDEX ASYNC`.
  The migration runner executes one statement per transaction and records it in
  `fp_schema_migrations`.
- **Types and sizes.** `text`/`json` values are limited to 1 MiB after compression (a post body
  over that fails loudly at save). Index keys are limited to 1 KiB, a table to 24 indexes, a row
  to 2 MiB. There is no `unsigned` (`bigint` covers the range). Date columns are `varchar(19)`, not
  `timestamp`, because the reference stores the zero date `0000-00-00 00:00:00`
  (for example a draft's GMT date) and domain crates read raw bytes. Fixed-width `YYYY-MM-DD hh:mm:ss`
  sorts chronologically as text.
- **Collation is `C` only.** The reference compares case-insensitively (its `_ci` collations), so
  login, email, option name, slug and meta-key lookups compare `lower(col) = lower($1)` over a
  lower-cased index. Whether DSQL takes expression indexes is verified in Phase 1. If it does
  not, a lower-cased shadow column is used.
- **No temp tables, no PL/pgSQL, no triggers.** The outbox row is written explicitly in the same
  transaction as the change, never by a trigger.
- **Tenancy:** 10 schemas and 1,000 tables per cluster, so table-prefix-per-site (about 12
  tables each) fits about 80 sites per cluster.
- **Auth:** IAM auth tokens (`dsql:DbConnect` with a non-admin role, 15-minute tokens). There is
  no database password to store. Connections last at most 60 minutes and are reused across warm
  invocations, then reconnected.

### DynamoDB: hot read model

One read table (`pk`, `sk`, on-demand, PITR on) and two small TTL tables:

| Item | Key | Contents |
|---|---|---|
| Route | `ROUTE#<normalized path>` / `R` | kind, status, content type, ETag, Last-Modified, S3 keys (body, shell), `version` |
| Published post doc | `POST#<id>` / `D` | denormalized: title, slug, permalink, dates, author display name, term ids/slugs, excerpt, featured image, body fragment key, `version` |
| Term listing | `TERM#<taxonomy>#<slug>` / `<post_date_gmt>#<id>` | one row per published post in the term, newest first by `Query ScanIndexForward=false`, used by the projector to render archive pages without scanning DSQL |
| Slug redirect | `ROUTE#<old path>` / `R` with kind `redirect` | the reference's old-slug redirect behavior |
| Import checkpoint | `IMPORT#<upload key>` / `C` | last committed batch |
| Session (sessions table) | `S#<sha256(token)>` | user id, created, expires (TTL), user-agent hash |
| Rate limit (limits table) | `RL#<route>#<ip>#<minute>` | counter, TTL 10 minutes |
| Transient / persistent cache (limits table) | `T#<site>#<name>` | value bytes, TTL from the transient expiry |

The `version` attribute is `<post_modified_gmt>#<outbox id>`. Writes are conditional on
`attribute_not_exists(version) OR version < :new`, so a late or retried projector run can never
overwrite newer output.

### S3: media and pre-rendered output

One bucket per site environment (block public access, SSE-S3, versioning on, OAC-only access):

- `site/uploads/YYYY/MM/<file>`: media, at the reference's upload URL layout so that existing
  image URLs keep working. Immutable; served `Cache-Control: public, max-age=31536000, immutable`.
- `site/rendered/<sha256>.html|.xml`: page bodies, feeds and sitemaps, content-addressed so a
  rewrite never mutates an object that a cached response or a route item points to.
- `site/fragments/shell/<sha256>.html`: theme shell parts.
- `site/search/<sha256>.json`: the prebuilt search index, with its current key named by the
  `SEARCH` route item.
- `site/incoming/`, `site/imports/`: upload landing areas, lifecycle-expired after 1 day.
- Lifecycle: unreferenced `rendered/` objects expire after 30 days. A route keeps one previous
  version for rollback.

## 5. Write propagation: outbox, projection, invalidation

1. A write Lambda (`admin`, `comments`, `media-process`, `cron`, `import`) opens one DSQL transaction:
   the row changes, plus one `fp_outbox` row (`id` uuid, `created_at`, `aggregate` such as `post`,
   `term`, `option`, `comment`, `user` or `theme`, `aggregate_id`, `change`, `payload` json).
   The commit includes the outbox row atomically with the change; this is the only coupling.
2. After commit it async-invokes `project` with an empty payload (a nudge). The 5-minute schedule
   is the safety net if a nudge is lost.
3. `project` runs at reserved concurrency 1, so projection is serial and ordered.
   1. Read up to 100 outbox rows in `created_at` order.
   2. Compute the affected route set. A post change affects:
      - its permalink and its old permalinks;
      - home pages 1..n;
      - the post's term archives, date archives and author archive;
      - its feeds, its sitemap shard and the search index.
   3. Render each route.
   4. Write the S3 objects (content-addressed, so a retry is harmless), then DynamoDB route and
      doc items (conditional on `version`), then one batched CloudFront invalidation for the
      changed paths.
   5. Delete the processed outbox rows.

   Delivery is at least once, and every step is idempotent.
4. **Ordering guarantee:** S3 is written before the DynamoDB pointer flips, and DynamoDB before the
   invalidation. A reader therefore never sees a pointer to a missing object, and a refetch after
   an invalidation always finds the new pointer.
5. **Consistency:** published output is eventually consistent, typically a few seconds plus
   CloudFront invalidation propagation. Editors get read-your-writes through previews. `admin`
   renders previews from DSQL directly at `/blog/admin/preview/<id>`, which is uncached and never
   shares a cache key with public pages.
6. **Theme, menu or option changes** that affect every page re-render the shell fragment (one
   object) and flip the shell key in each route item, a bounded DynamoDB batch. The invalidation
   is then one `/blog/*` wildcard path.
7. **Lag alarm:** `project` publishes the age of the oldest unprocessed outbox row. More than
   5 minutes alarms.

DSQL also offers CDC streams (the quotas page lists 5 per cluster). The CDC documentation page did
not render when read; Phase 5 evaluates whether a stream can replace the nudge-and-poll trigger.
The outbox stays either way, because it carries domain intent ("post 12 published"), not row
images.

Rendering theme output needs roadmap steps 24–25 (themes, blocks, formatting). Until those
land, the projector produces what is native core and needs no theme: feeds, sitemaps, robots,
redirects and the post doc JSON. HTML pages follow when the renderer exists. Nothing serves
placeholder HTML.

## 6. Edge

- **CloudFront in front of Lambda function URLs with Origin Access Control** (`AuthType AWS_IAM`).
  There is no API Gateway, since function URLs cost nothing beyond Lambda. OAC signs POST/PUT
  only when the viewer sends `x-amz-content-sha256`. The admin, comment and upload clients are
  Ferropress's own JavaScript and compute it with WebCrypto. API Gateway HTTP API is the
  fallback if a no-JS form path is required.
- **Mounting.** The CDK edge construct exports origins and behaviors, so the blog can be mounted
  either at a path prefix (`/blog/*`) on an existing distribution or as its own distribution on a
  subdomain. The base path comes from the `home` option, not from the Host header. Function URLs
  need their own Host, so behaviors use the managed `AllViewerExceptHostHeader` origin request
  policy.
- **Behaviors (path-prefix mount), in precedence order:**

| Pattern | Origin | Methods | Cache policy |
|---|---|---|---|
| `blog/wp-content/uploads/*` | S3 (OAC) | GET, HEAD | CachingOptimized, 1 year |
| `blog/admin/*`, `blog/api/*`, `blog/login*` | admin / auth URLs | all | CachingDisabled |
| `blog/comments*` | comments URL | all | CachingDisabled |
| `blog/*` | read URL (search URL when query `s` is present, routed by a CloudFront Function) | GET, HEAD | custom `fp-public` |
| `blog` | read URL | GET, HEAD | CachingDisabled (301 to `/blog/`) |

- **`fp-public` cache policy:**
  - Cache key: path plus an allowlist of query strings (`p`, `page_id`, `paged`, `cat`, `tag`,
    `author`, `m`, `feed`), no cookies, and `Accept-Encoding` normalized (gzip, br).
  - TTLs: min 0, default 1 day, max 1 year. Freshness comes from invalidation, not short TTLs;
    the first 1,000 invalidation paths a month are free and a blog uses tens.
  - The read Lambda sends `Cache-Control: public, max-age=60, s-maxage=86400`, so browsers
    revalidate quickly and the edge holds long.
- **Logged-in users see the public cached page.** There is no admin bar on public pages, and the
  comment form is prefilled client-side from cookies. This is a documented divergence from the
  reference, which varies public pages by cookie.
- **Response headers policy:** HSTS, `X-Content-Type-Options`, `Referrer-Policy`, and
  frame-ancestors `self` on admin.

## 7. Auth and sessions

- **Login** is in `auth`. It verifies existing reference password hashes natively (the bcrypt
  form and the legacy portable form) so migrated users keep their passwords, and rehashes to the
  current form on login. It is rate limited per IP and per login in DynamoDB with a conditional
  `UpdateItem` counter and TTL.
- **Session token:** 256 random bits in cookie `fp_session` (`HttpOnly; Secure; SameSite=Lax;
  Path=/blog/`). The server stores only `sha256(token)` in the sessions table with a TTL.
  - Logout deletes the item, and "log out everywhere" queries a `user_id` GSI.
  - The reference keeps session tokens in usermeta. Plugins that read that usermeta see an
    empty list, a documented divergence for the serverless profile.
- **Nonces:** the reference's time-tick HMAC nonce semantics, computed natively from the session
  token and the site salts.
- **Secrets:** salts and keys live in one Secrets Manager secret per site, read once at init by
  `auth`, `admin` and `comments`. Nothing secret goes into environment variables, logs or the
  repository. DSQL uses IAM tokens and needs no password.
- **Capabilities:** `cms-auth` as today, loaded from DSQL usermeta per admin request.

## 8. Migrating the existing data layer

ADR-0004 requires byte-exact MySQL statements, because plugins observe SQL text. DSQL speaks
PostgreSQL. The port keeps both:

1. **`cms_db::Dialect`** (`MySql`, `Postgres`) owns identifier quoting, string literal quoting
   and the insert-id mechanism. The `MySql` output stays byte-identical, and every existing
   statement test still passes unchanged.
2. Each crate's `sql.rs` takes the dialect. Postgres statements quote the reference's column
   names exactly (`"ID"`, `"post_name"`), and the DDL creates them quoted. Result-set column names
   therefore match, and row parsers such as `cms_content::post::from_set` stay unchanged.
3. **`infra-dsql`** implements `cms_db::Store` over the PostgreSQL wire protocol with TLS and IAM
   token auth. It returns the same `Reply` (text cells, affected rows from the command tag,
   insert id from `RETURNING "ID"`) and maps SQLSTATEs into `ServerError`.
4. **ADR-0010** records the serverless profile. MySQL remains the parity profile. Under DSQL,
   plugin raw SQL through the PHP-visible database object is **not supported**, since a
   MySQL-to-PostgreSQL translator is a separate decision. Certification is reported per profile,
   as ADR-0005 already does for isolation. Differential scenarios run against the DSQL profile
   compare results and database diffs; SQL text differs by design.

## 9. WXR import

1. Upload the export file to `site/imports/<name>.xml`. The `import` Lambda then:
   - stream-parses it (bounded memory);
   - upserts authors, then terms, then posts, postmeta and comments, in DSQL transactions of at
     most 2,500 rows or 8 MiB, checkpointing after each batch so a 15-minute timeout resumes;
   - keeps source IDs where they are free (identity `BY DEFAULT`) and moves the identity past the
     maximum imported id at the end;
   - stays idempotent by `guid` plus post type.
2. **Attachments** are fetched from their source URLs, or taken from an uploaded uploads archive,
   into `site/uploads/` at the same relative paths. Content URLs are rewritten only when the host
   changes.
3. One `rebuild-all` outbox event at the end makes `project` render every route.

## 10. Observability

- **Logs:** structured JSON through `tracing`, redacted per `rust-docs-observability.mdc` (no
  cookies, tokens, passwords or post passwords). CloudWatch Logs retention is 30 days.
- **Metrics:** CloudWatch EMF from each function:
  - read: route kind, hit/miss/304/404, DynamoDB and S3 latency;
  - projector: outbox age, routes rendered, invalidation paths;
  - DSQL: OCC retry count.
- **Alarms (SNS to email):**
  - read Lambda error rate above 1% or p99 above 500 ms;
  - CloudFront 5xx above 1%;
  - outbox age above 5 minutes;
  - async-invoke failures on the on-failure destination SQS queue;
  - a budget alarm at USD 10 a month.
- **One CloudWatch dashboard** per site environment. CloudFront standard logs go to S3 and supply
  the measured traffic table in section 2.

## 11. Cost at blog scale

Assumptions: 100k page views a month, about 1M edge requests including assets, about 30 GB out,
a 90% CloudFront hit ratio on HTML, 20 posts a month, and 2 GB of media. Prices are AWS list
prices as remembered, us-east-1, to be re-read before the first deploy.

| Item | Usage | Monthly |
|---|---|---|
| CloudFront | 1M requests, 30 GB, within the always-free 1 TB / 10M requests | $0 |
| Lambda | about 15k read misses × 20 ms × 128 MB, plus jobs; within the free tier | $0 |
| DynamoDB on-demand | about 50k reads, 5k writes, under 1 GB | under $0.10 |
| S3 | about 3 GB, about 100k GETs | about $0.10 |
| Aurora DSQL | editor-only writes and job reads, within the monthly free DPU and 1 GB allowance | $0–1 |
| CloudWatch | logs, about 10 metrics, 5 alarms, 1 dashboard | $2–4 |
| Secrets Manager | 1 secret | $0.40 |
| **Total** | | **about $3–6 a month** |

The dashboard and custom metrics dominate. At 10× the traffic the total stays under $10.

## 12. Infrastructure as code

AWS CDK in TypeScript under `deploy/aws/cdk/`, with three stacks per site environment:

- `Data`: the DSQL cluster, DynamoDB tables and S3 bucket. Retain on delete.
- `Compute`: the functions, roles (one per function, least privilege as in the table above), the
  schedules and the SQS on-failure queue.
- `Edge`: a construct that adds origins and behaviors to a given distribution or creates one.

The Rust binaries are built by CI for `aarch64-unknown-linux-musl` as `bootstrap` zips, which CDK
consumes as assets. The build does not run on developer laptops. Deployment is a separate,
owner-authorized step and outside this work.

## 13. Phases

Each phase is one tracked issue and reaches the repository quality gate before it closes.

1. **Data-access abstraction and DSQL schema:** `cms_db::Dialect`, dialect-aware builders in every
   `sql.rs`, `deploy/aws/dsql/0001-core.sql`, ADR-0010.
2. **Tiny read Lambda skeleton:** `cms-readmodel` (route keys, route items, response assembly behind
   `RouteTable` and `Blobs` ports) and `apps/lambda-read` (Runtime API loop).
3. **AWS adapters:** `infra-aws` (SigV4, DynamoDB `GetItem`/`PutItem`/`Query`, S3, CloudFront
   invalidation). An HTTPS client dependency is needed and is a dependency-admission review.
4. **`infra-dsql` store adapter:** PostgreSQL wire protocol, IAM tokens, OCC retry, migration
   runner.
5. **Outbox and projector job:** feeds, sitemaps, redirects and post docs first, HTML pages when the
   renderer exists. Evaluates DSQL CDC.
6. **CDK IaC:** Data, Compute and Edge stacks; synthesizes only, with no deploy.
7. **Auth and sessions Lambda.**
8. **Admin and editor Lambda:** depends on roadmap Phase 7.
9. **Media upload and processing.**
10. **Comments and forms with rate limits.**
11. **Search Lambda and prebuilt index.**
12. **WXR import job.**
13. **Observability, cost guardrails and measured traffic.**
14. **Edge mount and cutover runbook:** path prefix or subdomain.
