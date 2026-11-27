//! Typed records read from one WXR channel child.

use crate::error::ParseError;
use crate::tree::Node;

/// Per-row allowance for column names and punctuation when sizing a batch.
const ROW_OVERHEAD: u64 = 512;

/// One `wp:author` entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Author {
    /// `wp:author_id` in the source site.
    pub id: u64,
    /// `wp:author_login`.
    pub login: String,
    /// `wp:author_email`.
    pub email: String,
    /// `wp:author_display_name`.
    pub display_name: String,
    /// `wp:author_first_name`.
    pub first_name: String,
    /// `wp:author_last_name`.
    pub last_name: String,
}

/// One meta key/value pair (post, term or comment meta).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Meta {
    /// `wp:meta_key`.
    pub key: String,
    /// `wp:meta_value`.
    pub value: String,
}

/// One term declared in the channel (`wp:category`, `wp:tag` or `wp:term`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Term {
    /// `wp:term_id` in the source site.
    pub id: u64,
    /// `category`, `post_tag` or the custom taxonomy name.
    pub taxonomy: String,
    /// The term slug.
    pub slug: String,
    /// The display name.
    pub name: String,
    /// The parent term's slug, empty for a top-level term.
    pub parent: String,
    /// The description.
    pub description: String,
    /// `wp:termmeta` pairs.
    pub meta: Vec<Meta>,
}

/// A term an item is filed under (`<category domain=".." nicename="..">`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TermRef {
    /// The `domain` attribute: the taxonomy.
    pub taxonomy: String,
    /// The `nicename` attribute: the slug.
    pub slug: String,
    /// The element text: the display name.
    pub name: String,
}

/// One `wp:comment` on an item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Comment {
    /// `wp:comment_id` in the source site.
    pub id: u64,
    /// `wp:comment_author`.
    pub author: String,
    /// `wp:comment_author_email`.
    pub author_email: String,
    /// `wp:comment_author_url`.
    pub author_url: String,
    /// `wp:comment_author_IP`.
    pub author_ip: String,
    /// `wp:comment_date`.
    pub date: String,
    /// `wp:comment_date_gmt`.
    pub date_gmt: String,
    /// `wp:comment_content`.
    pub content: String,
    /// `wp:comment_approved`.
    pub approved: String,
    /// `wp:comment_type`.
    pub kind: String,
    /// `wp:comment_parent`: a source comment id.
    pub parent: u64,
    /// `wp:comment_user_id`: a source author id.
    pub user_id: u64,
    /// `wp:commentmeta` pairs.
    pub meta: Vec<Meta>,
}

/// One `item`: a post of any type with its meta, term links and comments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Item {
    /// `title`.
    pub title: String,
    /// `guid`: with the post type, the identity re-runs match on.
    pub guid: String,
    /// `dc:creator`: the author's login.
    pub creator: String,
    /// `content:encoded`.
    pub content: String,
    /// `excerpt:encoded`.
    pub excerpt: String,
    /// `wp:post_id` in the source site.
    pub id: u64,
    /// `wp:post_date`.
    pub date: String,
    /// `wp:post_date_gmt`.
    pub date_gmt: String,
    /// `wp:post_modified`, or the post date when the export has none.
    pub modified: String,
    /// `wp:post_modified_gmt`, or the GMT post date when the export has none.
    pub modified_gmt: String,
    /// `wp:comment_status`.
    pub comment_status: String,
    /// `wp:ping_status`.
    pub ping_status: String,
    /// `wp:post_name`.
    pub name: String,
    /// `wp:status`.
    pub status: String,
    /// `wp:post_parent`: a source post id.
    pub parent: u64,
    /// `wp:menu_order`.
    pub menu_order: u64,
    /// `wp:post_type`.
    pub kind: String,
    /// `wp:post_password`.
    pub password: String,
    /// `wp:post_mime_type`.
    pub mime_type: String,
    /// `wp:attachment_url`, empty unless this is an attachment.
    pub attachment_url: String,
    /// Term links.
    pub terms: Vec<TermRef>,
    /// `wp:postmeta` pairs.
    pub meta: Vec<Meta>,
    /// Comments.
    pub comments: Vec<Comment>,
}

/// One channel child the importer acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Record {
    /// `wp:base_site_url`: the source site's address.
    Site(String),
    /// An author.
    Author(Author),
    /// A declared term.
    Term(Term),
    /// An item.
    Item(Box<Item>),
}

impl Record {
    /// Rows and bytes this record writes when imported, an upper estimate used to close a batch
    /// before it would exceed its limits.
    #[must_use]
    pub fn weight(&self) -> (u64, u64) {
        match self {
            Self::Site(_) => (0, 0),
            Self::Author(author) => {
                let bytes = author.login.len() * 2 + author.email.len() + author.display_name.len();
                (1, to_u64(bytes) + ROW_OVERHEAD)
            }
            Self::Term(term) => {
                let rows = 2 + term.meta.len();
                let bytes = term.name.len()
                    + term.slug.len()
                    + term.description.len()
                    + meta_bytes(&term.meta);
                (to_u64(rows), to_u64(bytes) + to_u64(rows) * ROW_OVERHEAD)
            }
            Self::Item(item) => {
                let comment_rows: usize = item.comments.iter().map(|c| 1 + c.meta.len()).sum();
                // Each linked term may also be created on the fly (two rows) besides its link.
                let rows = 2 + item.meta.len() + item.terms.len() * 3 + comment_rows;
                let comment_bytes: usize = item
                    .comments
                    .iter()
                    .map(|c| {
                        c.content.len() + c.author.len() + c.author_url.len() + meta_bytes(&c.meta)
                    })
                    .sum();
                let term_bytes: usize = item
                    .terms
                    .iter()
                    .map(|t| t.slug.len() * 2 + t.name.len())
                    .sum();
                let bytes = item.content.len()
                    + item.title.len()
                    + item.excerpt.len()
                    + item.guid.len()
                    + item.name.len()
                    + meta_bytes(&item.meta)
                    + comment_bytes
                    + term_bytes;
                (to_u64(rows), to_u64(bytes) + to_u64(rows) * ROW_OVERHEAD)
            }
        }
    }
}

fn meta_bytes(meta: &[Meta]) -> usize {
    meta.iter().map(|pair| pair.key.len() + pair.value.len()).sum()
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// A non-negative integer field: surrounding whitespace is ignored and empty means zero.
fn number(node: &Node, field: &'static str) -> Result<u64, ParseError> {
    let text = node.field(field).trim();
    if text.is_empty() {
        return Ok(0);
    }
    text.parse::<u64>().map_err(|_| ParseError::Number {
        field,
        value: Box::from(text),
    })
}

fn owned(node: &Node, field: &str) -> String {
    node.field(field).to_owned()
}

fn metas(node: &Node, element: &str) -> Vec<Meta> {
    node.children(element)
        .map(|pair| Meta {
            key: owned(pair, "wp:meta_key"),
            value: owned(pair, "wp:meta_value"),
        })
        .collect()
}

/// An author from a `wp:author` element.
pub(crate) fn author(node: &Node) -> Result<Author, ParseError> {
    Ok(Author {
        id: number(node, "wp:author_id")?,
        login: owned(node, "wp:author_login"),
        email: owned(node, "wp:author_email"),
        display_name: owned(node, "wp:author_display_name"),
        first_name: owned(node, "wp:author_first_name"),
        last_name: owned(node, "wp:author_last_name"),
    })
}

/// A term from a `wp:category`, `wp:tag` or `wp:term` element.
pub(crate) fn term(node: &Node) -> Result<Term, ParseError> {
    // Each declaration form names the same facts with its own element names.
    let (taxonomy, slug, parent, name, description) = match node.name() {
        "wp:category" => (
            "category".to_owned(),
            "wp:category_nicename",
            "wp:category_parent",
            "wp:cat_name",
            "wp:category_description",
        ),
        "wp:tag" => (
            "post_tag".to_owned(),
            "wp:tag_slug",
            "",
            "wp:tag_name",
            "wp:tag_description",
        ),
        _ => (
            owned(node, "wp:term_taxonomy"),
            "wp:term_slug",
            "wp:term_parent",
            "wp:term_name",
            "wp:term_description",
        ),
    };
    Ok(Term {
        id: number(node, "wp:term_id")?,
        taxonomy,
        slug: owned(node, slug),
        name: owned(node, name),
        parent: if parent.is_empty() {
            String::new()
        } else {
            owned(node, parent)
        },
        description: owned(node, description),
        meta: metas(node, "wp:termmeta"),
    })
}

fn comment(node: &Node) -> Result<Comment, ParseError> {
    Ok(Comment {
        id: number(node, "wp:comment_id")?,
        author: owned(node, "wp:comment_author"),
        author_email: owned(node, "wp:comment_author_email"),
        author_url: owned(node, "wp:comment_author_url"),
        author_ip: owned(node, "wp:comment_author_IP"),
        date: owned(node, "wp:comment_date"),
        date_gmt: owned(node, "wp:comment_date_gmt"),
        content: owned(node, "wp:comment_content"),
        approved: owned(node, "wp:comment_approved"),
        kind: owned(node, "wp:comment_type"),
        parent: number(node, "wp:comment_parent")?,
        user_id: number(node, "wp:comment_user_id")?,
        meta: metas(node, "wp:commentmeta"),
    })
}

/// An item from an `item` element.
pub(crate) fn item(node: &Node) -> Result<Item, ParseError> {
    let date = owned(node, "wp:post_date");
    let date_gmt = owned(node, "wp:post_date_gmt");
    // Exports carry no modified date; the reference importer stamps the post date there.
    let modified = node
        .children("wp:post_modified")
        .next()
        .map_or_else(|| date.clone(), |field| field.text().to_owned());
    let modified_gmt = node
        .children("wp:post_modified_gmt")
        .next()
        .map_or_else(|| date_gmt.clone(), |field| field.text().to_owned());
    let terms = node
        .children("category")
        .filter_map(|link| {
            // Links without a domain are a pre-1.1 form that names no taxonomy.
            let taxonomy = link.attr("domain")?;
            Some(TermRef {
                taxonomy: taxonomy.to_owned(),
                slug: link.attr("nicename").unwrap_or_default().to_owned(),
                name: link.text().to_owned(),
            })
        })
        .collect();
    let comments = node
        .children("wp:comment")
        .map(comment)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Item {
        title: owned(node, "title"),
        guid: owned(node, "guid"),
        creator: owned(node, "dc:creator"),
        content: owned(node, "content:encoded"),
        excerpt: owned(node, "excerpt:encoded"),
        id: number(node, "wp:post_id")?,
        date,
        date_gmt,
        modified,
        modified_gmt,
        comment_status: owned(node, "wp:comment_status"),
        ping_status: owned(node, "wp:ping_status"),
        name: owned(node, "wp:post_name"),
        status: owned(node, "wp:status"),
        parent: number(node, "wp:post_parent")?,
        menu_order: number(node, "wp:menu_order")?,
        kind: owned(node, "wp:post_type"),
        password: owned(node, "wp:post_password"),
        mime_type: owned(node, "wp:post_mime_type"),
        attachment_url: owned(node, "wp:attachment_url"),
        terms,
        meta: metas(node, "wp:postmeta"),
        comments,
    })
}
