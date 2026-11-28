//! Previews: the post as the public theme would render it, from the DSQL row, never cached.

use cms_render::{Author, CardSource, Post, PostKind, Stamp, Term, shell, single};

use crate::Error;
use crate::date::format;
use crate::editor::Site;
use crate::model::{PostRow, TermRow};

/// Renders `row` with the built-in theme's shell and single-post fragment.
///
/// A draft carries the zero date, which the theme cannot show, so an unparsable date renders as
/// `now` (owner decision recorded on #9). The canonical link points at the preview URL itself so
/// no public URL is guessed before the projector assigns one.
///
/// # Errors
///
/// [`Error::Rejected`] when the card source fails or the shell lacks a composition mark.
pub(crate) fn render(
    site: &Site,
    row: &PostRow,
    terms: &[TermRow],
    author: Option<(String, String)>,
    now: i64,
    cards: &mut dyn CardSource,
) -> Result<Vec<u8>, Error> {
    let theme = &site.theme;
    let stamp = |stored: &str, fallback: String| {
        Stamp::parse(stored.as_bytes())
            .or_else(|_zero| Stamp::parse(fallback.as_bytes()))
            .map_err(|error| Error::Rejected(error.to_string().into()))
    };
    let (name, nicename) = author.unwrap_or_default();
    let linked = |taxonomy: &str, path: &str| -> Vec<Term> {
        terms
            .iter()
            .filter(|term| term.taxonomy == taxonomy)
            .map(|term| Term {
                name: term.name.as_str().into(),
                slug: term.slug.as_str().into(),
                link: theme.url(&format!("{path}/{}/", term.slug)).into(),
            })
            .collect()
    };
    let permalink: Box<str> = theme
        .url(&format!("admin/preview/{}", row.id.as_u64()))
        .into();
    let post = Post {
        id: row.id.as_u64(),
        kind: PostKind::Post,
        title: row.title.as_str().into(),
        slug: row.name.as_str().into(),
        guid: permalink.clone(),
        permalink,
        date: stamp(&row.date, site.local(now))?,
        date_gmt: stamp(&row.date_gmt, format(now))?,
        modified: stamp(&row.modified, site.local(now))?,
        modified_gmt: stamp(&row.modified_gmt, format(now))?,
        author: Author {
            link: theme.url(&format!("author/{nicename}/")).into(),
            name: name.into(),
            slug: nicename.into(),
        },
        categories: linked("category", "category"),
        tags: linked("post_tag", "tag"),
        excerpt: row.excerpt.as_str().into(),
        content: row.content.as_str().into(),
    };
    let fragment = single(theme, &post, cards).map_err(|error| Error::Rejected(error.0.clone()))?;
    cms_readmodel::assemble(&shell(theme).bytes, &fragment.rendered.bytes)
        .map_err(|error| Error::Rejected(error.to_string().into()))
}
