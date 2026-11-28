//! The DSQL source over a fake store that answers each of its three statements by table.

use cms_db::{Cell, Column, Dialect, Reply, ResultSet, Row, Sql, Store, StoreError, TablePrefix};
use cms_render::PostKind;

use super::DbSource;
use crate::{PortError, Source};

type Outcome = Result<(), Box<dyn std::error::Error>>;

struct Tables {
    options: Vec<Row>,
    terms: Vec<Row>,
    posts: Vec<Row>,
}

impl Store for Tables {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn query(&mut self, sql: &Sql) -> Result<Reply, StoreError> {
        let text = String::from_utf8_lossy(sql.as_bytes());
        let rows = if text.contains("\"fp_options\"") {
            self.options.clone()
        } else if text.contains("\"fp_term_relationships\"") {
            self.terms.clone()
        } else {
            self.posts.clone()
        };
        Ok(Reply::Rows(ResultSet::new(vec![Column::new("c")], rows)))
    }
}

fn row(cells: &[Option<&str>]) -> Row {
    Row::new(
        cells
            .iter()
            .map(|cell| cell.map_or(Cell::Null, Cell::bytes))
            .collect::<Vec<_>>(),
    )
}

fn option(name: &str, value: &str) -> Row {
    row(&[Some(name), Some(value)])
}

// One posts-table row: id, type, slug, parent, local date, author nicename.
fn post(id: &str, kind: &str, slug: &str, parent: &str, date: &str, author: Option<&str>) -> Row {
    row(&[
        Some(id),
        Some(kind),
        Some("Title"),
        Some(slug),
        Some("guid"),
        Some(date),
        Some(date),
        Some(date),
        Some(date),
        Some(""),
        Some("Body"),
        Some(parent),
        Some("Ann"),
        author,
    ])
}

fn tables(structure: &str, posts: Vec<Row>) -> Tables {
    Tables {
        options: vec![
            option("blogname", "Example"),
            option("blogdescription", "Notes"),
            option("home", "https://example.com/blog"),
            option("posts_per_page", "5"),
            option("permalink_structure", structure),
            option("WPLANG", "de_DE"),
        ],
        terms: vec![
            row(&[Some("7"), Some("category"), Some("News"), Some("news")]),
            row(&[Some("7"), Some("post_tag"), Some("Rust"), Some("rust")]),
        ],
        posts,
    }
}

#[test]
fn site_settings_come_from_the_options() -> Outcome {
    let mut source = DbSource::new(tables("/%postname%/", Vec::new()), TablePrefix::new("fp_")?);
    let site = source.site()?;
    assert_eq!(&*site.home, "https://example.com/blog/");
    assert_eq!(&*site.base, "/blog/");
    assert_eq!(&*site.language, "de-DE");
    assert_eq!(site.posts_per_page, 5);
    assert_eq!(&*site.title, "Example");
    Ok(())
}

#[test]
fn posts_get_permalinks_terms_and_authors_and_zero_dates_are_left_out() -> Outcome {
    let rows = vec![
        post("7", "post", "hello", "0", "2026-03-07 09:05:01", Some("ann")),
        post("8", "page", "about", "0", "2026-01-01 00:00:00", Some("ann")),
        post("9", "page", "team", "8", "2026-01-01 00:00:00", Some("ann")),
        post("10", "post", "draftish", "0", "0000-00-00 00:00:00", Some("ann")),
    ];
    let structure = "/%year%/%monthnum%/%day%/%category%/%postname%-%post_id%/";
    let mut source = DbSource::new(tables(structure, rows), TablePrefix::new("fp_")?);
    let posts = source.published()?;
    assert_eq!(posts.len(), 3);
    let hello = &posts[0];
    assert_eq!(
        &*hello.permalink,
        "https://example.com/blog/2026/03/07/news/hello-7/"
    );
    assert_eq!(&*hello.categories[0].link, "https://example.com/blog/category/news/");
    assert_eq!(&*hello.tags[0].link, "https://example.com/blog/tag/rust/");
    assert_eq!(&*hello.author.link, "https://example.com/blog/author/ann/");
    assert_eq!(posts[1].kind, PostKind::Page);
    assert_eq!(&*posts[2].permalink, "https://example.com/blog/about/team/");
    Ok(())
}

#[test]
fn plain_permalinks_and_missing_authors_fail_hard() -> Outcome {
    let mut plain = DbSource::new(tables("", Vec::new()), TablePrefix::new("fp_")?);
    assert_eq!(
        plain.published(),
        Err(PortError(
            "plain permalinks are not supported by the serverless profile".into()
        ))
    );
    let orphan = vec![post("7", "post", "hello", "0", "2026-03-07 09:05:01", None)];
    let mut source = DbSource::new(tables("/%postname%/", orphan), TablePrefix::new("fp_")?);
    assert_eq!(
        source.published(),
        Err(PortError("post 7 has no author row".into()))
    );
    let mut relative = tables("/%postname%/", Vec::new());
    relative.options[2] = option("home", "/blog");
    let mut source = DbSource::new(relative, TablePrefix::new("fp_")?);
    assert!(source.site().is_err());
    source.refresh();
    assert!(source.site().is_err());
    Ok(())
}
