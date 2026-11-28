//! Sitemaps at the reference's core sitemap paths, and robots.txt.
//!
//! The index is `wp-sitemap.xml`; each provider shard is `wp-sitemap-<provider>-<n>.xml`, at most
//! [`URLS_PER_SITEMAP`] URLs each. Providers with no URLs are left out of the index. The
//! reference's XSL stylesheet line is not written (this profile serves no stylesheet).

use crate::escape::push_escaped;
use crate::model::Site;
use crate::theme::Rendered;

/// URLs per sitemap shard, the reference's limit.
pub const URLS_PER_SITEMAP: usize = 2000;

/// The sitemap index file name, relative to the home URL.
pub const SITEMAP_INDEX: &str = "wp-sitemap.xml";

/// `Content-Type` of sitemaps.
pub const XML: &str = "application/xml; charset=UTF-8";

/// `Content-Type` of robots.txt.
pub const TEXT: &str = "text/plain; charset=UTF-8";

const NAMESPACE: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";

/// Absolute URLs per provider, in listing order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SitemapUrls {
    /// Published posts' permalinks.
    pub posts: Vec<Box<str>>,
    /// Published pages' permalinks; the home URL is listed first automatically.
    pub pages: Vec<Box<str>>,
    /// Category archive URLs.
    pub categories: Vec<Box<str>>,
    /// Tag archive URLs.
    pub tags: Vec<Box<str>>,
    /// Author archive URLs.
    pub users: Vec<Box<str>>,
}

/// One sitemap file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SitemapFile {
    /// File name relative to the home URL, such as `wp-sitemap-posts-post-1.xml`.
    pub path: Box<str>,
    /// The XML.
    pub rendered: Rendered,
}

/// The index followed by every shard.
#[must_use]
pub fn sitemaps(site: &Site, urls: &SitemapUrls) -> Vec<SitemapFile> {
    // The reference lists the home page at the head of the page provider.
    let mut pages = Vec::with_capacity(urls.pages.len() + 1);
    pages.push(&*site.home);
    pages.extend(urls.pages.iter().map(|url| &**url));
    let providers: [(&str, Vec<&str>); 5] = [
        ("posts-post", urls.posts.iter().map(|u| &**u).collect()),
        ("posts-page", pages),
        (
            "taxonomies-category",
            urls.categories.iter().map(|u| &**u).collect(),
        ),
        (
            "taxonomies-post_tag",
            urls.tags.iter().map(|u| &**u).collect(),
        ),
        ("users", urls.users.iter().map(|u| &**u).collect()),
    ];
    let mut shards = Vec::new();
    for (provider, list) in &providers {
        for (index, chunk) in list.chunks(URLS_PER_SITEMAP).enumerate() {
            shards.push(SitemapFile {
                path: format!("wp-sitemap-{provider}-{}.xml", index + 1).into(),
                rendered: urlset(chunk),
            });
        }
    }
    let mut index = String::with_capacity(128 + shards.len() * 96);
    index.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"");
    index.push_str(NAMESPACE);
    index.push_str("\">");
    for shard in &shards {
        index.push_str("<sitemap><loc>");
        push_escaped(&mut index, &site.url(&shard.path));
        index.push_str("</loc></sitemap>");
    }
    index.push_str("</sitemapindex>\n");
    let mut files = Vec::with_capacity(shards.len() + 1);
    files.push(SitemapFile {
        path: SITEMAP_INDEX.into(),
        rendered: Rendered::new(index, XML),
    });
    files.extend(shards);
    files
}

fn urlset(urls: &[&str]) -> Rendered {
    let mut out = String::with_capacity(128 + urls.len() * 80);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"");
    out.push_str(NAMESPACE);
    out.push_str("\">");
    for url in urls {
        out.push_str("<url><loc>");
        push_escaped(&mut out, url);
        out.push_str("</loc></url>");
    }
    out.push_str("</urlset>\n");
    Rendered::new(out, XML)
}

/// robots.txt: the admin path disallowed and the sitemap index named.
#[must_use]
pub fn robots(site: &Site) -> Rendered {
    let text = format!(
        "User-agent: *\nDisallow: {}admin/\n\nSitemap: {}\n",
        site.base,
        site.url(SITEMAP_INDEX)
    );
    Rendered::new(text, TEXT)
}
