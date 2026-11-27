Definition of finished (phase 3):

1. `chore(deps)`: ureq's `rustls` feature (ring provider, webpki roots) admitted in the root Cargo.toml with the rust-dependencies-tooling.mdc checklist recorded on this issue; any deny.toml change is its own chore commit with a WHY comment, scoped to the named crates.
2. New crate `crates/infra-aws` (infrastructure layer, `[lints] workspace = true`, workspace dependency versions only):
   - `sigv4`: header signing and query-string presigning over sha2 with a native HMAC-SHA256; tests transcribe AWS's published SigV4 suite vectors (at least get-vanilla, get-vanilla-query-order-key-case, get-header-value-trim, post-vanilla, post-x-www-form-urlencoded, get-vanilla-with-session-token) and assert canonical request, string to sign and authorization header; HMAC checked against RFC 4231 vectors.
   - Aurora DSQL IAM auth token builder (`DbConnect` / `DbConnectAdmin`, service `dsql`, 900 s default) built on the presigner, with a test that fixes clock and credentials.
   - `Credentials` read from an explicit environment lookup (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, optional AWS_SESSION_TOKEN, AWS_REGION); no global state; missing variables are typed errors.
   - DynamoDB JSON-protocol client: GetItem, PutItem conditional on `attribute_not_exists(version) OR version < :new` (condition failure is a distinct outcome, not an error), DeleteItem, Query (pk equality, descending, limit, pagination via LastEvaluatedKey).
   - S3 GetObject (404 maps to `None`), PutObject (content-type, cache-control), DeleteObject.
   - CloudFront CreateInvalidation (XML body, caller reference).
   - Every endpoint overridable so tests point at 127.0.0.1; HTTPS by default.
   - Read adapters implementing `cms_readmodel::RouteTable` and `cms_readmodel::Blobs`, plus a writer API for the phase 5 projector (route/doc put conditional on version, delete, term-listing query, S3 put, invalidation).
   - Tests: in-process fake HTTP servers on 127.0.0.1 that assert method, path, signed headers and payload, and return canned responses; every public fn and error path covered; no AWS calls.
3. `apps/lambda-read` gets `src/main.rs`: env config (base path, table, bucket, region, credentials, Runtime API) -> adapters -> existing `run` loop, with an immutable content-keyed shell-fragment cache kept across warm invocations (tested).
4. Release binary size for the host measured and reported here.
5. Per-crate gate green for infra-aws, cms-readmodel and lambda-read: fmt --check, clippy --all-targets --all-features -D warnings, test --all-features; llvm-cov run if available.
