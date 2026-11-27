Progress: `5b7793f` on branch `p11-search` (worktree `ferropress-wt/p11-search`) adds `crates/cms-search` (document model, index builder, term parser, matcher and ordering) with 14 contract cases plus unit tests. fmt, clippy `-D warnings` and tests pass for `-p cms-search`.

Next step: `apps/lambda-search` (Function URL event, index cache keyed by the `SEARCH` route item, HTML results page, REST search JSON), then `deploy/aws/cdk/functions/route-search.js` with its `node --test` test.

Owner decision (default taken): no dependency admission is needed. cms-search uses only serde, serde_json, sha2 and thiserror, which are already workspace dependencies.
Owner decision (default taken): matching is case-insensitive through Unicode lower-casing. The reference's `_ci` collation is also accent-insensitive, so a search for `cafe` does not match `café` here. This is a documented divergence.
Owner decision (default taken): content is indexed with HTML tags and comments removed (block delimiters included), as the phase brief asks. A search for a tag or block name therefore does not match every post the way the reference's raw-content match does. This is a documented divergence.
Owner decision (default taken): when relevance rank and date are equal, the newer id comes first. The reference leaves that order to the database.
Owner decision (default taken): the projector indexes only published posts without a password. The search Lambda is always anonymous at the edge, the same as a logged-out visitor.
Owner decision (default taken): reference-lab comparison is replaced by the contract tests in `crates/cms-search/tests/contract.rs`, because the lab needs Docker. The lab differential run is a follow-up.
