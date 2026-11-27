//! The outbox-age metric as a `CloudWatch` embedded metric format (EMF) log line.

use serde_json::json;

/// One EMF JSON line recording the age of the oldest unprocessed outbox row and how many rows
/// this run projected. Written to stdout, `CloudWatch` Logs extracts the metrics with no API
/// call, which the lag alarm (more than 300 seconds) watches.
#[must_use]
pub fn emf_line(now_micros: i64, age_seconds: u64, projected: usize) -> String {
    json!({
        "_aws": {
            "Timestamp": now_micros.max(0) / 1000,
            "CloudWatchMetrics": [{
                "Namespace": "Ferropress/Projector",
                "Dimensions": [[]],
                "Metrics": [
                    {"Name": "OutboxAgeSeconds", "Unit": "Seconds"},
                    {"Name": "Projected", "Unit": "Count"}
                ]
            }]
        },
        "OutboxAgeSeconds": age_seconds,
        "Projected": projected
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::emf_line;

    #[test]
    fn the_line_is_emf_json() {
        assert_eq!(
            emf_line(1_500_000, 42, 3),
            r#"{"OutboxAgeSeconds":42,"Projected":3,"_aws":{"CloudWatchMetrics":[{"Dimensions":[[]],"Metrics":[{"Name":"OutboxAgeSeconds","Unit":"Seconds"},{"Name":"Projected","Unit":"Count"}],"Namespace":"Ferropress/Projector"}],"Timestamp":1500}}"#
        );
    }
}
