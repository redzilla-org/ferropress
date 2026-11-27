# ADR-0010: Serverless storage profile (Aurora DSQL, DynamoDB, S3)

- **Status:** Proposed. The owner ordered the AWS Lambda port (issue #1). Acceptance follows phase 4
  (#5), when differential scenarios run on the DSQL profile.
- **Date:** 2026-11-27

## Context

ADR-0004 makes the reference MySQL schema and the exact MySQL statement text part of the contract,
with `infra-mysql` as the only adapter. The AWS Lambda deployment (`docs/aws/architecture.md`) uses
Aurora DSQL as the relational source of truth. DSQL speaks PostgreSQL and has different quoting,
collation (`C` only), types (no zero dates, no unsigned) and transaction limits (3,000 rows, 10 MiB,
5 minutes, optimistic concurrency). Its hot read path is a DynamoDB read model plus pre-rendered
S3 objects. MySQL statement text cannot run there unchanged.

## Decision

- **Two storage profiles.** `mysql` (ADR-0004, the parity profile, unchanged) and `serverless`
  (this ADR). Compatibility and certification are reported per profile, as ADR-0005 does for
  isolation.
- **Same schema shape.** The serverless profile keeps the reference tables, column names, defaults
  and indexes (`deploy/aws/dsql/0001-core.sql`, rendered from the recorded inventory), adapted only
  where DSQL forces it. It adds two tables, `fp_outbox` and `fp_schema_migrations`.
- **Dialect at the statement builders.** `cms_db::Dialect` spells identifiers and literals. The
  `MySql` output stays byte-identical to the reference. Each crate's `sql.rs` takes the dialect, so
  domain crates and row parsers are shared across profiles.
- **`infra-dsql`** implements `cms_db::Store` with the same `Reply` shape. SQL text differs by
  design, so differential scenarios on this profile compare results and database diffs, not
  statement text.
- **Plugin raw SQL through the PHP-visible database object is not supported** on the serverless
  profile. A MySQL-to-PostgreSQL translator would need its own ADR and parity evidence.
- **State that does not belong in DSQL moves out:**
  - transients and the persistent object cache go to DynamoDB with TTL;
  - sessions go to DynamoDB, so the session-tokens usermeta stays empty;
  - scheduled events are owned by the cron schedule, not the `cron` option row.

  These are documented divergences.

## Consequences

- Every statement builder gains a dialect parameter. This is one mechanical change per crate,
  verified by the existing byte-exact MySQL tests plus new PostgreSQL tests.
- Case-insensitive reference lookups need explicit `lower()` comparisons on the serverless
  profile.
- Writes must be retried on optimistic-concurrency conflicts, and bulk paths must chunk under the
  transaction limits.

## Verification

- The MySQL statement tests in every crate stay unchanged and green.
- The `store_contract` passes for `infra-dsql`.
- Differential scenarios on the serverless profile compare results and database diffs against
  the reference lab.
