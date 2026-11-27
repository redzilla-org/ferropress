//! Route classes of the traffic table (architecture section 2), derived from a request path.

/// The class of a request, as counted in the traffic table and logged as `route_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouteClass {
    /// A single post (`2026/11/some-post/` or `?p=`).
    Post,
    /// Home and paged home (the empty path, `page/2/`).
    Home,
    /// Any feed (`feed/`, `category/x/feed/`, `?feed=`).
    Feed,
    /// Term, date and author archives.
    Archive,
    /// Sitemaps and `robots.txt`.
    Sitemap,
    /// A page (`about/` or `?page_id=`).
    Page,
    /// Search (`?s=`).
    Search,
    /// Comment and form posts.
    Comments,
    /// Admin, login and REST writes.
    Admin,
    /// Uploads and static files; served from S3, never by a Lambda.
    Asset,
}

/// Archive prefixes under the base path.
const ARCHIVE_PREFIXES: &[&str] = &["category/", "tag/", "author/"];
/// Query variables that select an archive.
const ARCHIVE_QUERY: &[&str] = &["cat", "tag", "author", "m"];

impl RouteClass {
    /// Every class in traffic-table order.
    pub const ALL: [Self; 10] = [
        Self::Post,
        Self::Home,
        Self::Feed,
        Self::Archive,
        Self::Sitemap,
        Self::Page,
        Self::Search,
        Self::Comments,
        Self::Admin,
        Self::Asset,
    ];

    /// The class of a request for `path` (relative to the base path, no leading `/`) with the raw
    /// `query` string (empty when absent).
    ///
    /// Posts are recognized by the `year/month/slug/` permalink structure the projector writes.
    ///
    /// # Examples
    ///
    /// ```
    /// use cms_telemetry::RouteClass;
    ///
    /// assert_eq!(RouteClass::classify("2026/11/some-post/", ""), RouteClass::Post);
    /// assert_eq!(RouteClass::classify("", "s=condos"), RouteClass::Search);
    /// ```
    #[must_use]
    pub fn classify(path: &str, query: &str) -> Self {
        let path = path.trim_start_matches('/');
        let has = |name: &str| {
            query
                .split('&')
                .any(|pair| pair.split_once('=').map_or(pair, |(key, _)| key) == name)
        };
        // Fixed prefixes first: they own their whole subtree whatever the query says.
        if ["admin/", "api/", "login"]
            .iter()
            .any(|p| path.starts_with(p))
        {
            return Self::Admin;
        }
        if path.starts_with("comments") {
            return Self::Comments;
        }
        if path.starts_with("wp-content/") {
            return Self::Asset;
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        let xml = last
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("xml"));
        if last.eq_ignore_ascii_case("robots.txt") || (xml && last.contains("sitemap")) {
            return Self::Sitemap;
        }
        if last.contains('.') {
            return Self::Asset;
        }
        if has("s") {
            return Self::Search;
        }
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.contains(&"feed") || has("feed") {
            return Self::Feed;
        }
        if has("p") {
            return Self::Post;
        }
        if has("page_id") {
            return Self::Page;
        }
        if ARCHIVE_PREFIXES.iter().any(|p| path.starts_with(p))
            || ARCHIVE_QUERY.iter().any(|q| has(q))
        {
            return Self::Archive;
        }
        // Drop a trailing `page/N` so paged views classify like their first page.
        let unpaged = match segments.as_slice() {
            [head @ .., "page", n] if is_number(n) => head,
            all => all,
        };
        match unpaged {
            [] => Self::Home,
            [year, month, slug] if is_number(year) && is_number(month) && !is_number(slug) => {
                Self::Post
            }
            [year, rest @ ..]
                if is_number(year) && rest.len() <= 2 && rest.iter().all(|s| is_number(s)) =>
            {
                Self::Archive
            }
            _ => Self::Page,
        }
    }

    /// The `route_kind` value logged and the row label key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Home => "home",
            Self::Feed => "feed",
            Self::Archive => "archive",
            Self::Sitemap => "sitemap",
            Self::Page => "page",
            Self::Search => "search",
            Self::Comments => "comments",
            Self::Admin => "admin",
            Self::Asset => "asset",
        }
    }
}

fn is_number(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::RouteClass::{
        self, Admin, Archive, Asset, Comments, Feed, Home, Page, Post, Search, Sitemap,
    };

    #[test]
    fn paths_and_queries_classify() {
        let cases: &[(&str, &str, RouteClass)] = &[
            ("2026/11/some-post/", "", Post),
            ("/2026/11/some-post/page/2/", "", Post),
            ("", "p=12", Post),
            ("", "", Home),
            ("page/3/", "", Home),
            ("feed/", "", Feed),
            ("category/news/feed/", "", Feed),
            ("", "feed=rss2", Feed),
            ("category/news/", "", Archive),
            ("tag/x/page/2/", "", Archive),
            ("author/ann/", "", Archive),
            ("2026/", "", Archive),
            ("2026/11/", "", Archive),
            ("2026/11/27/", "", Archive),
            ("", "cat=3", Archive),
            ("", "m=202611", Archive),
            ("wp-sitemap.xml", "", Sitemap),
            ("wp-sitemap-posts-post-1.xml", "", Sitemap),
            ("robots.txt", "", Sitemap),
            ("about/", "", Page),
            ("about/team/", "", Page),
            ("", "page_id=2", Page),
            ("", "s=condos&paged=2", Search),
            ("comments", "", Comments),
            ("comments/post", "", Comments),
            ("admin/posts", "s=x", Admin),
            ("api/posts", "", Admin),
            ("login", "", Admin),
            ("wp-content/uploads/2026/11/a.jpg", "", Asset),
            ("favicon.ico", "", Asset),
        ];
        for (path, query, class) in cases {
            assert_eq!(RouteClass::classify(path, query), *class, "{path}?{query}");
        }
    }

    #[test]
    fn every_class_has_a_distinct_name() {
        let names: std::collections::BTreeSet<&str> =
            RouteClass::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), RouteClass::ALL.len());
        assert_eq!(Post.as_str(), "post");
        assert_eq!(Asset.as_str(), "asset");
    }
}
