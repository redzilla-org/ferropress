//! Route shares measured from `CloudFront` standard logs (W3C extended format, tab-separated).
//!
//! The logs are downloaded and decompressed beforehand (see `docs/aws/observability.md`); this
//! tool only reads files, classifies each request under the base path with
//! [`RouteClass::classify`], and prints the Markdown table that replaces the estimate in the
//! architecture document.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use cms_telemetry::RouteClass;

/// Why a run failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Bad command line.
    #[error("{0}; usage: traffic-share [--base /blog/] FILE...")]
    Usage(&'static str),
    /// A log file could not be read, or the table could not be written.
    #[error("{path}: {source}")]
    Io {
        /// File (or `stdout`) involved.
        path: String,
        /// The underlying error.
        source: io::Error,
    },
    /// A log file is not in the standard-log format.
    #[error("{path}:{line}: {reason}")]
    Format {
        /// File being read.
        path: String,
        /// 1-based line number.
        line: usize,
        /// What was wrong.
        reason: &'static str,
    },
}

/// Fields the tally needs from the `#Fields:` header.
const STEM: &str = "cs-uri-stem";
const QUERY: &str = "cs-uri-query";
const RESULT: &str = "x-edge-result-type";

/// Request counts per route class, plus what was left out of the table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Per class: (requests, requests answered from the edge cache).
    classes: BTreeMap<RouteClass, (u64, u64)>,
    /// Uploads and static files under the base path.
    assets: u64,
    /// Requests outside the base path (the host site's own traffic).
    outside: u64,
}

impl Tally {
    /// Counts one request for `stem` (the URI path) and raw `query`, `hit` if served from cache.
    pub fn record(&mut self, base: &str, stem: &str, query: &str, hit: bool) {
        // The bare prefix without its slash (`/blog`) is the redirect to the home page.
        let relative = match stem.strip_prefix(base) {
            Some(rest) => rest,
            None if format!("{stem}/") == base => "",
            None => {
                self.outside += 1;
                return;
            }
        };
        let class = RouteClass::classify(relative, query);
        if class == RouteClass::Asset {
            self.assets += 1;
            return;
        }
        let entry = self.classes.entry(class).or_default();
        entry.0 += 1;
        entry.1 += u64::from(hit);
    }

    /// Adds every request in one standard-log stream; `path` names it in errors.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] on a read failure and [`Error::Format`] on a data line before the `#Fields:`
    /// header, a header missing a needed field, or a line with the wrong column count.
    pub fn read<R: BufRead>(&mut self, base: &str, path: &str, reader: R) -> Result<(), Error> {
        let mut columns: Option<(usize, usize, usize, usize)> = None;
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|source| Error::Io {
                path: path.to_owned(),
                source,
            })?;
            let format = |reason| Error::Format {
                path: path.to_owned(),
                line: index + 1,
                reason,
            };
            if let Some(header) = line.strip_prefix("#Fields:") {
                let names: Vec<&str> = header.split_whitespace().collect();
                let find = |name| names.iter().position(|field| *field == name);
                columns = match (find(STEM), find(QUERY), find(RESULT)) {
                    (Some(stem), Some(query), Some(result)) => {
                        Some((names.len(), stem, query, result))
                    }
                    _ => {
                        return Err(format(
                            "header lacks cs-uri-stem, cs-uri-query or x-edge-result-type",
                        ));
                    }
                };
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (width, stem, query, result) =
                columns.ok_or_else(|| format("data before the #Fields header"))?;
            let cells: Vec<&str> = line.split('\t').collect();
            if cells.len() != width {
                return Err(format("column count differs from the #Fields header"));
            }
            // An absent query is logged as `-`.
            let raw_query = if cells[query] == "-" {
                ""
            } else {
                cells[query]
            };
            let hit = matches!(cells[result], "Hit" | "RefreshHit");
            self.record(base, cells[stem], raw_query, hit);
        }
        Ok(())
    }

    /// Writes the route-share table as Markdown.
    ///
    /// # Errors
    ///
    /// Any error from `out`.
    pub fn write_table<W: Write>(&self, base: &str, out: &mut W) -> io::Result<()> {
        let total: u64 = self.classes.values().map(|(requests, _)| requests).sum();
        let hits: u64 = self.classes.values().map(|(_, hit)| hit).sum();
        writeln!(out, "| Route class | Requests | Share | Edge hit ratio |")?;
        writeln!(out, "|---|---|---|---|")?;
        for class in RouteClass::ALL {
            if class == RouteClass::Asset {
                continue;
            }
            let (requests, hit) = self.classes.get(&class).copied().unwrap_or_default();
            writeln!(
                out,
                "| {} | {requests} | {} | {} |",
                label(class),
                percent(requests, total),
                percent(hit, requests)
            )?;
        }
        writeln!(
            out,
            "| **Total** | {total} | {} | {} |",
            percent(total, total),
            percent(hits, total)
        )?;
        writeln!(out)?;
        writeln!(
            out,
            "Excluded: {} upload/static requests under {base}, {} requests outside {base}.",
            self.assets, self.outside
        )
    }
}

/// Row label, matching the traffic table in the architecture document.
const fn label(class: RouteClass) -> &'static str {
    match class {
        RouteClass::Post => "Single post",
        RouteClass::Home => "Home and paged home",
        RouteClass::Feed => "Feeds",
        RouteClass::Archive => "Term / date / author archives",
        RouteClass::Sitemap => "Sitemaps, robots",
        RouteClass::Page => "Pages",
        RouteClass::Search => "Search",
        RouteClass::Comments => "Comments / forms POST",
        RouteClass::Admin => "Admin, login, REST writes",
        RouteClass::Asset => "Uploads and static files",
    }
}

/// `part / whole` as a percentage with one decimal, rounded half up; `-` when `whole` is 0.
fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "-".to_owned();
    }
    let tenths = (part * 1000 + whole / 2) / whole;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// Runs the command line `args` (without the program name), writing the table to `out`.
///
/// # Errors
///
/// [`Error::Usage`] for a bad command line, otherwise as [`Tally::read`] and
/// [`Tally::write_table`].
pub fn run<W: Write>(args: &[String], out: &mut W) -> Result<(), Error> {
    let (base, files) = match args {
        [flag, base, files @ ..] if flag == "--base" => (base.as_str(), files),
        [flag, ..] if flag == "--base" => return Err(Error::Usage("--base needs a value")),
        files => ("/blog/", files),
    };
    if !base.starts_with('/') || !base.ends_with('/') {
        return Err(Error::Usage("the base path must start and end with /"));
    }
    if files.is_empty() {
        return Err(Error::Usage("no log files given"));
    }
    let mut tally = Tally::default();
    for path in files {
        let file = File::open(path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        tally.read(base, path, BufReader::new(file))?;
    }
    tally.write_table(base, out).map_err(|source| Error::Io {
        path: "stdout".to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{Error, Tally, percent, run};

    const HEADER: &str =
        "#Version: 1.0\n#Fields: date cs-uri-stem cs-uri-query x-edge-result-type\n";

    fn read(text: &str) -> Result<Tally, Error> {
        let mut tally = Tally::default();
        tally.read("/blog/", "t.log", text.as_bytes())?;
        Ok(tally)
    }

    #[test]
    fn percentages_round_half_up_to_one_decimal() {
        assert_eq!(percent(7, 18), "38.9%");
        assert_eq!(percent(1, 18), "5.6%");
        assert_eq!(percent(1, 1), "100.0%");
        assert_eq!(percent(0, 0), "-");
    }

    #[test]
    fn the_bare_prefix_counts_as_home_and_other_paths_as_outside() {
        let mut tally = Tally::default();
        tally.record("/blog/", "/blog", "", true);
        tally.record("/blog/", "/shop/", "", false);
        tally.record("/", "/about/", "", false);
        let mut out = Vec::new();
        assert!(tally.write_table("/blog/", &mut out).is_ok());
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("| Home and paged home | 1 | 50.0% | 100.0% |"),
            "{text}"
        );
        assert!(text.contains("| Pages | 1 | 50.0% | 0.0% |"), "{text}");
        assert!(text.contains("1 requests outside /blog/"), "{text}");
    }

    #[test]
    fn data_before_the_header_is_rejected() {
        let error = read("2026-11-20\t/blog/\t-\tHit\n").map_err(|error| error.to_string());
        assert_eq!(
            error.err().as_deref(),
            Some("t.log:1: data before the #Fields header")
        );
    }

    #[test]
    fn a_header_without_needed_fields_is_rejected() {
        let error = read("#Fields: date time\n").map_err(|error| error.to_string());
        assert_eq!(
            error.err().as_deref(),
            Some("t.log:1: header lacks cs-uri-stem, cs-uri-query or x-edge-result-type")
        );
    }

    #[test]
    fn short_lines_are_rejected() {
        let error =
            read(&format!("{HEADER}2026-11-20\t/blog/\n")).map_err(|error| error.to_string());
        assert_eq!(
            error.err().as_deref(),
            Some("t.log:3: column count differs from the #Fields header")
        );
    }

    #[test]
    fn blank_and_comment_lines_are_skipped() {
        let tally = read(&format!("{HEADER}\n#note\n2026-11-20\t/blog/\t-\tMiss\n")).ok();
        let mut expected = Tally::default();
        expected.record("/blog/", "/blog/", "", false);
        assert_eq!(tally, Some(expected));
    }

    #[test]
    fn read_errors_name_the_file() {
        struct Broken;
        impl io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disk gone"))
            }
        }
        let error = Tally::default()
            .read("/blog/", "t.log", io::BufReader::new(Broken))
            .map_err(|error| error.to_string());
        assert_eq!(error, Err("t.log: disk gone".to_owned()));
    }

    #[test]
    fn bad_command_lines_are_usage_errors() {
        let usage = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(ToString::to_string).collect();
            run(&args, &mut Vec::new())
                .map_err(|error| error.to_string())
                .err()
        };
        let suffix = "; usage: traffic-share [--base /blog/] FILE...";
        assert_eq!(usage(&[]), Some(format!("no log files given{suffix}")));
        assert_eq!(
            usage(&["--base"]),
            Some(format!("--base needs a value{suffix}"))
        );
        assert_eq!(
            usage(&["--base", "blog", "x.log"]),
            Some(format!("the base path must start and end with /{suffix}"))
        );
        assert!(
            usage(&["/nonexistent/fp-traffic.log"])
                .is_some_and(|message| message.starts_with("/nonexistent/fp-traffic.log: "))
        );
    }
}
