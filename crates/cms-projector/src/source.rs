//! The [`Source`] adapter over any [`Store`]: site options, published posts with their authors,
//! and their categories and tags, in three statements per run.

use std::collections::HashMap;

use cms_db::{Cell, Reply, Row, Statement, Store, TablePrefix, TableStem};
use cms_render::{Author, DEFAULT_POSTS_PER_PAGE, Post, PostKind, Site, Stamp, Term};

use crate::{PortError, Source};

/// Options the projector reads; anything else is the write paths' concern.
const OPTIONS: [&str; 6] = [
    "blogname",
    "blogdescription",
    "home",
    "posts_per_page",
    "permalink_structure",
    "WPLANG",
];

/// Loads current state with plain SELECTs on its own connection, outside any transaction.
#[derive(Debug)]
pub struct DbSource<S: Store> {
    store: S,
    prefix: TablePrefix,
    // Options are read once per source, since `published` needs the home URL and structure.
    options: Option<HashMap<Box<str>, Box<str>>>,
}

impl<S: Store> DbSource<S> {
    /// A source reading the tables under `prefix` through `store`.
    pub const fn new(store: S, prefix: TablePrefix) -> Self {
        Self {
            store,
            prefix,
            options: None,
        }
    }

    /// Forgets cached options, so the next run sees changes. The projector job calls this per run.
    pub fn refresh(&mut self) {
        self.options = None;
    }

    fn table(&self, stem: &str) -> Result<Vec<u8>, PortError> {
        let stem = TableStem::new(stem).map_err(|error| port("table name", &error))?;
        Ok(self.prefix.table(&stem).as_bytes().to_vec())
    }

    fn select(&mut self, sql: Statement) -> Result<Vec<Row>, PortError> {
        let sql = sql.finish().map_err(|error| port("source statement", &error))?;
        match self.store.query(&sql).map_err(|error| port("source store", &error))? {
            Reply::Rows(set) => Ok(set.rows().to_vec()),
            Reply::Failed(error) => Err(PortError(
                format!("source query failed: {}", error.message()).into(),
            )),
            _ => Err(PortError("source query returned no rows".into())),
        }
    }

    fn options(&mut self) -> Result<HashMap<Box<str>, Box<str>>, PortError> {
        if let Some(options) = &self.options {
            return Ok(options.clone());
        }
        let mut sql = Statement::new(self.store.dialect());
        sql.raw("SELECT ")
            .ident("option_name")
            .raw(", ")
            .ident("option_value")
            .raw(" FROM ")
            .ident(self.table("options")?)
            .raw(" WHERE ")
            .ident("option_name")
            .raw(" IN (");
        for (index, name) in OPTIONS.iter().enumerate() {
            if index > 0 {
                sql.raw(", ");
            }
            sql.literal(name);
        }
        sql.raw(")");
        let mut options = HashMap::new();
        for row in self.select(sql)? {
            options.insert(text(&row, 0)?, text(&row, 1)?);
        }
        self.options = Some(options.clone());
        Ok(options)
    }

    // Categories and tags per post id, in name order within each taxonomy.
    fn terms(&mut self, home: &str) -> Result<HashMap<u64, (Vec<Term>, Vec<Term>)>, PortError> {
        let mut sql = Statement::new(self.store.dialect());
        sql.raw("SELECT r.")
            .ident("object_id")
            .raw(", x.")
            .ident("taxonomy")
            .raw(", t.")
            .ident("name")
            .raw(", t.")
            .ident("slug")
            .raw(" FROM ")
            .ident(self.table("term_relationships")?)
            .raw(" r JOIN ")
            .ident(self.table("term_taxonomy")?)
            .raw(" x ON x.")
            .ident("term_taxonomy_id")
            .raw(" = r.")
            .ident("term_taxonomy_id")
            .raw(" JOIN ")
            .ident(self.table("terms")?)
            .raw(" t ON t.")
            .ident("term_id")
            .raw(" = x.")
            .ident("term_id")
            .raw(" WHERE x.")
            .ident("taxonomy")
            .raw(" IN (")
            .literal("category")
            .raw(", ")
            .literal("post_tag")
            .raw(") ORDER BY r.")
            .ident("object_id")
            .raw(", t.")
            .ident("name");
        let mut terms: HashMap<u64, (Vec<Term>, Vec<Term>)> = HashMap::new();
        for row in self.select(sql)? {
            let (id, taxonomy) = (number(&row, 0)?, text(&row, 1)?);
            let slug = text(&row, 3)?;
            let base = if &*taxonomy == "post_tag" { "tag" } else { "category" };
            let term = Term {
                name: text(&row, 2)?,
                link: format!("{home}{base}/{slug}/").into(),
                slug,
            };
            let entry = terms.entry(id).or_default();
            if base == "tag" {
                entry.1.push(term);
            } else {
                entry.0.push(term);
            }
        }
        Ok(terms)
    }
}

impl<S: Store> Source for DbSource<S> {
    fn site(&mut self) -> Result<Site, PortError> {
        let options = self.options()?;
        let get = |name: &str| options.get(name).cloned().unwrap_or_default();
        let home = home(&get("home"))?;
        let base = base_of(&home)?;
        // `WPLANG` is a locale such as `de_DE`; empty means the reference's default, en-US.
        let language = match &*get("WPLANG") {
            "" => "en-US".into(),
            locale => locale.replace('_', "-").into(),
        };
        let posts_per_page = get("posts_per_page")
            .parse()
            .unwrap_or(DEFAULT_POSTS_PER_PAGE);
        Ok(Site {
            title: get("blogname"),
            description: get("blogdescription"),
            home,
            base,
            language,
            posts_per_page,
            nav: Vec::new(),
        })
    }

    fn published(&mut self) -> Result<Vec<Post>, PortError> {
        let options = self.options()?;
        let home = home(options.get("home").map_or("", |home| home))?;
        let structure = options
            .get("permalink_structure")
            .cloned()
            .unwrap_or_default();
        // Plain `?p=<id>` links have no path to key a route by.
        if structure.is_empty() {
            return Err(PortError(
                "plain permalinks are not supported by the serverless profile".into(),
            ));
        }
        let mut terms = self.terms(&home)?;
        let mut sql = Statement::new(self.store.dialect());
        sql.raw("SELECT ");
        for (index, column) in [
            "ID",
            "post_type",
            "post_title",
            "post_name",
            "guid",
            "post_date",
            "post_date_gmt",
            "post_modified",
            "post_modified_gmt",
            "post_excerpt",
            "post_content",
            "post_parent",
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                sql.raw(", ");
            }
            sql.raw("p.").ident(column);
        }
        sql.raw(", u.")
            .ident("display_name")
            .raw(", u.")
            .ident("user_nicename")
            .raw(" FROM ")
            .ident(self.table("posts")?)
            .raw(" p LEFT JOIN ")
            .ident(self.table("users")?)
            .raw(" u ON u.")
            .ident("ID")
            .raw(" = p.")
            .ident("post_author")
            .raw(" WHERE p.")
            .ident("post_status")
            .raw(" = ")
            .literal("publish")
            .raw(" AND p.")
            .ident("post_type")
            .raw(" IN (")
            .literal("post")
            .raw(", ")
            .literal("page")
            .raw(") AND p.")
            .ident("post_password")
            .raw(" = ")
            .literal("");
        let rows = self.select(sql)?;
        // Page permalinks nest under their parents' slugs, so collect every page first.
        let parents: HashMap<u64, (u64, Box<str>)> = rows
            .iter()
            .filter(|row| text(row, 1).is_ok_and(|kind| &*kind == "page"))
            .map(|row| Ok((number(row, 0)?, (number(row, 11)?, text(row, 3)?))))
            .collect::<Result<_, PortError>>()?;
        let mut posts = Vec::with_capacity(rows.len());
        for row in &rows {
            // A published row with a zero date cannot be rendered; it is left out.
            let stamps = [5, 6, 7, 8].map(|index| text(row, index).map(|value| Stamp::parse(value.as_bytes())));
            let [Ok(Ok(date)), Ok(Ok(date_gmt)), Ok(Ok(modified)), Ok(Ok(modified_gmt))] = stamps
            else {
                continue;
            };
            let id = number(row, 0)?;
            let kind = if &*text(row, 1)? == "page" {
                PostKind::Page
            } else {
                PostKind::Post
            };
            let slug = text(row, 3)?;
            let nicename = text(row, 13)
                .map_err(|_| PortError(format!("post {id} has no author row").into()))?;
            let (categories, tags) = terms.remove(&id).unwrap_or_default();
            let author = Author {
                name: text(row, 12)?,
                link: format!("{home}author/{nicename}/").into(),
                slug: nicename,
            };
            let permalink = match kind {
                PostKind::Page => format!("{home}{}/", page_path(&parents, id)),
                PostKind::Post => {
                    let category = categories.first().map_or("uncategorized", |term| &term.slug);
                    let path = expand(&structure, id, &slug, date, &author.slug, category);
                    format!("{home}{}", path.trim_start_matches('/'))
                }
            };
            posts.push(Post {
                id,
                kind,
                title: text(row, 2)?,
                slug,
                permalink: permalink.into(),
                guid: text(row, 4)?,
                date,
                date_gmt,
                modified,
                modified_gmt,
                author,
                categories,
                tags,
                excerpt: text(row, 9)?,
                content: text(row, 10)?,
            });
        }
        Ok(posts)
    }
}

// The home URL with exactly one trailing slash; it must be absolute.
fn home(value: &str) -> Result<Box<str>, PortError> {
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err(PortError(format!("home option {value:?} is not an absolute URL").into()));
    }
    Ok(format!("{}/", value.trim_end_matches('/')).into())
}

// The path part of the home URL, such as `/blog/`.
fn base_of(home: &str) -> Result<Box<str>, PortError> {
    let after_scheme = home
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| PortError("home has no scheme".into()))?;
    Ok(after_scheme
        .find('/')
        .map_or("/", |at| &after_scheme[at..])
        .into())
}

// `parent/child` for a page, walking the parent chain (cycles are cut at the page count).
fn page_path(pages: &HashMap<u64, (u64, Box<str>)>, id: u64) -> String {
    let mut slugs = Vec::new();
    let mut at = id;
    while let Some((parent, slug)) = pages.get(&at) {
        slugs.push(&**slug);
        if *parent == 0 || slugs.len() > pages.len() {
            break;
        }
        at = *parent;
    }
    slugs.reverse();
    slugs.join("/")
}

// The reference's permalink structure tags, from the post's local date.
fn expand(structure: &str, id: u64, slug: &str, date: Stamp, author: &str, category: &str) -> String {
    let of_day = date.seconds().rem_euclid(86_400);
    let pairs = [
        ("%year%", format!("{:04}", date.year())),
        ("%monthnum%", format!("{:02}", date.month())),
        ("%day%", format!("{:02}", date.day())),
        ("%hour%", format!("{:02}", of_day / 3600)),
        ("%minute%", format!("{:02}", of_day % 3600 / 60)),
        ("%second%", format!("{:02}", of_day % 60)),
        ("%post_id%", id.to_string()),
        ("%postname%", slug.to_owned()),
        ("%author%", author.to_owned()),
        ("%category%", category.to_owned()),
    ];
    pairs
        .iter()
        .fold(structure.to_owned(), |path, (tag, value)| path.replace(tag, value))
}

fn text(row: &Row, index: usize) -> Result<Box<str>, PortError> {
    match row.cells().get(index) {
        Some(Cell::Bytes(bytes)) => std::str::from_utf8(bytes)
            .map(Into::into)
            .map_err(|error| port("source column", &error)),
        Some(Cell::Null) | None => Err(PortError(format!("source column {index} is missing").into())),
    }
}

fn number(row: &Row, index: usize) -> Result<u64, PortError> {
    text(row, index)?
        .parse()
        .map_err(|error| port("source number", &error))
}

fn port(what: &str, error: &dyn std::fmt::Display) -> PortError {
    PortError(format!("{what}: {error}").into())
}

#[cfg(test)]
mod tests;
