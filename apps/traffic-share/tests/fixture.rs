//! The route-share table for a synthetic `CloudFront` standard log.

/// Expected table for `fixtures/standard.log`: 20 requests, of which one upload and one host-site
/// page are excluded.
const EXPECTED: &str = "\
| Route class | Requests | Share | Edge hit ratio |
|---|---|---|---|
| Single post | 7 | 38.9% | 71.4% |
| Home and paged home | 2 | 11.1% | 50.0% |
| Feeds | 2 | 11.1% | 50.0% |
| Term / date / author archives | 1 | 5.6% | 0.0% |
| Sitemaps, robots | 2 | 11.1% | 50.0% |
| Pages | 1 | 5.6% | 100.0% |
| Search | 1 | 5.6% | 0.0% |
| Comments / forms POST | 1 | 5.6% | 0.0% |
| Admin, login, REST writes | 1 | 5.6% | 0.0% |
| **Total** | 18 | 100.0% | 50.0% |

Excluded: 1 upload/static requests under /blog/, 1 requests outside /blog/.
";

#[test]
fn the_fixture_log_yields_the_expected_table() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/standard.log");
    let args = vec!["--base".to_owned(), "/blog/".to_owned(), fixture.to_owned()];
    let mut out = Vec::new();
    let result = traffic_share::run(&args, &mut out).map_err(|error| error.to_string());
    assert_eq!(result, Ok(()));
    assert_eq!(String::from_utf8_lossy(&out), EXPECTED);
}
