//! Metric names shared by the emitting functions, the CDK alarms and the dashboard.
//!
//! The catalogue with units and emit rules is `docs/aws/observability.md`.

/// Read: a route item was served with a 2xx status. Count.
pub const READ_HIT: &str = "ReadHit";
/// Read: a conditional GET matched the route's validator (304). Count.
pub const READ_NOT_MODIFIED: &str = "ReadNotModified";
/// Read: a redirect was served (route redirect or added trailing slash). Count.
pub const READ_REDIRECT: &str = "ReadRedirect";
/// Read: no route item, or a removed route (404 or 410). Count.
pub const READ_NOT_FOUND: &str = "ReadNotFound";
/// Read: whole invocation time inside the handler. Milliseconds.
pub const READ_LATENCY: &str = "ReadLatency";
/// Read: one route-table lookup in `DynamoDB`. Milliseconds.
pub const DYNAMODB_GET_LATENCY: &str = "DynamoDbGetLatency";
/// Read: one blob fetch from S3. Milliseconds.
pub const S3_GET_LATENCY: &str = "S3GetLatency";
/// Projector: age of the oldest unprojected outbox row at the start of a run (0 when empty). Seconds.
pub const OUTBOX_AGE: &str = "OutboxAge";
/// Projector: route items written in one run. Count.
pub const ROUTES_RENDERED: &str = "RoutesRendered";
/// Projector: `CloudFront` invalidation paths submitted in one run. Count.
pub const INVALIDATION_PATHS: &str = "InvalidationPaths";
/// Any DSQL writer: transaction retries after an optimistic-concurrency conflict. Count.
pub const DSQL_OCC_RETRIES: &str = "DsqlOccRetries";

/// The read outcome metric for a response `status`, or `None` for errors (5xx and others),
/// which the platform's `Errors` metric already counts.
///
/// # Examples
///
/// ```
/// use cms_telemetry::names::{READ_NOT_MODIFIED, read_outcome};
///
/// assert_eq!(read_outcome(304), Some(READ_NOT_MODIFIED));
/// assert_eq!(read_outcome(502), None);
/// ```
#[must_use]
pub const fn read_outcome(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => Some(READ_HIT),
        304 => Some(READ_NOT_MODIFIED),
        300..=399 => Some(READ_REDIRECT),
        404 | 410 => Some(READ_NOT_FOUND),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{READ_HIT, READ_NOT_FOUND, READ_NOT_MODIFIED, READ_REDIRECT, read_outcome};

    #[test]
    fn statuses_map_to_outcomes() {
        assert_eq!(read_outcome(200), Some(READ_HIT));
        assert_eq!(read_outcome(304), Some(READ_NOT_MODIFIED));
        assert_eq!(read_outcome(301), Some(READ_REDIRECT));
        assert_eq!(read_outcome(308), Some(READ_REDIRECT));
        assert_eq!(read_outcome(404), Some(READ_NOT_FOUND));
        assert_eq!(read_outcome(410), Some(READ_NOT_FOUND));
        assert_eq!(read_outcome(400), None);
        assert_eq!(read_outcome(500), None);
    }
}
