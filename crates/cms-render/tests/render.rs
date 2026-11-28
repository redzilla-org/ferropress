//! Every output kind of the built-in theme over small fixtures, assembled as the read path does.

use cms_readmodel::assemble;
use cms_render::{
    Archive, ArchiveKind, Author, Card, CardError, CardSource, Channel, Facts, Link, ListingCard,
    Marker, NoCards, Post, PostKind, Rendered, Site, SitemapUrls, Stamp, Term, URLS_PER_SITEMAP,
    archive, excerpt, feed, not_found, page_count, robots, shell, single, sitemaps,
};

fn site() -> Site {
    Site {
        title: "Example & Co".into(),
        description: "Notes <daily>".into(),
        home: "https://example.com/blog/".into(),
        base: "/blog/".into(),
        language: "en-US".into(),
        posts_per_page: 10,
        nav: vec![Link {
            label: "About".into(),
            url: "https://example.com/blog/about/".into(),
        }],
    }
}

#[expect(
    clippy::expect_used,
    reason = "fixture helper; a bad fixture fails the test"
)]
fn stamp(text: &str) -> Stamp {
    Stamp::parse(text.as_bytes()).expect("fixture stamp")
}

fn term(name: &str, slug: &str, base: &str) -> Term {
    Term {
        name: name.into(),
        slug: slug.into(),
        link: format!("https://example.com/blog/{base}/{slug}/").into(),
    }
}

fn post(id: u64, content: &str) -> Post {
    Post {
        id,
        kind: PostKind::Post,
        title: format!("Post \"{id}\"").into(),
        slug: format!("post-{id}").into(),
        permalink: format!("https://example.com/blog/post-{id}/").into(),
        guid: format!("https://example.com/blog/?p={id}").into(),
        date: stamp("2026-11-27 10:00:00"),
        date_gmt: stamp("2026-11-27 18:00:00"),
        modified: stamp("2026-11-28 10:00:00"),
        modified_gmt: stamp("2026-11-28 18:00:00"),
        author: Author {
            name: "Ann Writer".into(),
            slug: "ann".into(),
            link: "https://example.com/blog/author/ann/".into(),
        },
        categories: vec![term("News", "news", "category")],
        tags: vec![term("Tips & Tricks", "tips", "tag")],
        excerpt: "".into(),
        content: content.into(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper; non-UTF-8 output fails the test"
)]
fn text(rendered: &Rendered) -> &str {
    std::str::from_utf8(&rendered.bytes).expect("utf-8 output")
}

struct OneCard;

impl CardSource for OneCard {
    fn card(&mut self, marker: &Marker) -> Result<Option<Card>, CardError> {
        Ok(match marker {
            Marker::Listing(id) => Some(Card::Listing(ListingCard {
                url: format!("https://listings.example.com/{id}").into(),
                photo: None,
                address: "1 Main St".into(),
                place: "".into(),
                price: 500_000,
                date: "".into(),
                facts: Facts::default(),
                credit: "".into(),
            })),
            Marker::Zip(_) => None,
        })
    }
}

#[test]
fn the_shell_carries_both_marks_and_the_site_chrome() {
    let shell = shell(&site());
    let html = text(&shell);
    assert_eq!(shell.content_type, "text/html; charset=UTF-8");
    assert!(
        html.starts_with(
            "<!DOCTYPE html>\n<html lang=\"en-US\">\n<head>\n<meta charset=\"UTF-8\">"
        )
    );
    assert!(html.contains("<link rel=\"alternate\" type=\"application/rss+xml\" title=\"Example &amp; Co \u{bb} Feed\" href=\"https://example.com/blog/feed/\">"));
    assert!(html.contains("<!--fp:head-->\n</head>"));
    assert!(html.contains("<p class=\"site-description\">Notes &lt;daily&gt;</p><nav><a href=\"https://example.com/blog/about/\">About</a></nav>"));
    assert!(html.contains("<main>\n<!--fp:body-->\n</main>"));
    let mut bare = site();
    bare.description = "".into();
    bare.nav.clear();
    let html = String::from_utf8(shell_of(&bare)).expect("utf-8");
    assert!(html.contains("Example &amp; Co</a></p></header>"));
}

fn shell_of(site: &Site) -> Vec<u8> {
    shell(site).bytes
}

#[test]
fn a_single_post_assembles_into_a_full_page() {
    let site = site();
    let post = post(
        7,
        "<!-- wp:paragraph -->\n<p>Hello <b>world</b>.</p>\n<!-- /wp:paragraph -->\n<p>[[AB12345]]</p>\n<p>[[92130]]</p>",
    );
    let out = single(&site, &post, &mut OneCard).expect("render");
    assert_eq!(out.unresolved, vec![Marker::Zip("92130".into())]);
    let page = assemble(&shell(&site).bytes, &out.rendered.bytes).expect("assemble");
    let html = String::from_utf8(page).expect("utf-8");
    assert!(html.contains("<title>Post &quot;7&quot; \u{2013} Example &amp; Co</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/post-7/\">\n<meta name=\"description\" content=\"Hello world.\">\n<script type=\"application/ld+json\">{\"@context\":\"https://schema.org\",\"@type\":\"BlogPosting\",\"headline\":\"Post \\\"7\\\"\""));
    assert!(html.contains("\"datePublished\":\"2026-11-27T10:00:00-08:00\",\"dateModified\":\"2026-11-28T10:00:00-08:00\""));
    assert!(html.contains("\"publisher\":{\"@type\":\"Organization\",\"name\":\"Example \\u0026 Co\",\"url\":\"https://example.com/blog/\"},\"description\":\"Hello world.\"}</script>\n\n</head>"));
    assert!(html.contains("<article class=\"post\">\n<header><h1>Post &quot;7&quot;</h1><p class=\"meta\"><time datetime=\"2026-11-27T10:00:00-08:00\">November 27, 2026</time> by <a href=\"https://example.com/blog/author/ann/\">Ann Writer</a></p></header>"));
    assert!(html.contains("<div class=\"content\">\n<p>Hello <b>world</b>.</p>\n<div class=\"fp-card fp-card-listing\">"));
    assert!(!html.contains("[["));
    assert!(html.contains("<footer><p class=\"meta\">Categories: <a href=\"https://example.com/blog/category/news/\">News</a></p><p class=\"meta\">Tags: <a href=\"https://example.com/blog/tag/tips/\">Tips &amp; Tricks</a></p></footer>"));
    assert!(!html.contains("<!--fp:"));
}

#[test]
fn a_page_has_no_byline_terms_or_blog_posting() {
    let mut page = post(2, "About us.");
    page.kind = PostKind::Page;
    page.excerpt = " Who we are ".into();
    let out = single(&site(), &page, &mut NoCards).expect("render");
    assert_eq!(
        text(&out.rendered),
        "<title>Post &quot;2&quot; \u{2013} Example &amp; Co</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/post-2/\">\n<meta name=\"description\" content=\"Who we are\">\n<!--fp:/head--><article class=\"page\">\n<header><h1>Post &quot;2&quot;</h1></header>\n<div class=\"content\">\n<p>About us.</p>\n</div>\n</article>\n"
    );
    let mut bare = post(3, "x");
    bare.categories.clear();
    bare.tags.clear();
    let out = single(&site(), &bare, &mut NoCards).expect("render");
    assert!(text(&out.rendered).ends_with("</div>\n</article>\n"));
}

#[test]
fn card_source_failures_fail_singles_and_feeds() {
    struct Down;
    impl CardSource for Down {
        fn card(&mut self, _marker: &Marker) -> Result<Option<Card>, CardError> {
            Err(CardError("down".into()))
        }
    }
    let post = post(1, "[[92130]]");
    assert!(single(&site(), &post, &mut Down).is_err());
    let items = [post];
    let channel = Channel {
        name: None,
        link: "https://example.com/blog/",
        self_url: "https://example.com/blog/feed/",
        items: &items,
    };
    assert!(feed(&site(), &channel, &mut Down).is_err());
}

#[test]
fn the_home_page_is_paged_with_newer_and_older_links() {
    let site = site();
    let items: Vec<Post> = (1..=3).map(|id| post(id, "Body text here.")).collect();
    assert_eq!(page_count(25, site.posts_per_page), 3);
    let first = archive(
        &site,
        &Archive {
            kind: &ArchiveKind::Home,
            url: &site.home,
            page: 1,
            pages: 3,
            items: &items[..1],
        },
    );
    assert_eq!(
        text(&first),
        "<title>Example &amp; Co \u{2013} Notes &lt;daily&gt;</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/\">\n<meta name=\"description\" content=\"Notes &lt;daily&gt;\">\n<!--fp:/head--><article class=\"post\"><h2><a href=\"https://example.com/blog/post-1/\">Post &quot;1&quot;</a></h2><p class=\"meta\"><time datetime=\"2026-11-27T10:00:00-08:00\">November 27, 2026</time></p><p>Body text here.</p><p><a href=\"https://example.com/blog/post-1/\">Continue reading</a></p></article>\n<nav class=\"pagination\"><span>Page 1 of 3</span> <a rel=\"next\" href=\"https://example.com/blog/page/2/\">Older posts</a></nav>\n"
    );
    let middle = archive(
        &site,
        &Archive {
            kind: &ArchiveKind::Home,
            url: &site.home,
            page: 2,
            pages: 3,
            items: &items[1..2],
        },
    );
    let html = text(&middle);
    assert!(html.starts_with("<title>Example &amp; Co \u{2013} Page 2 \u{2013} Notes &lt;daily&gt;</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/page/2/\">\n<!--fp:/head-->"));
    assert!(html.contains("<a rel=\"prev\" href=\"https://example.com/blog/\">Newer posts</a> <span>Page 2 of 3</span> <a rel=\"next\" href=\"https://example.com/blog/page/3/\">Older posts</a>"));
    let mut quiet = site.clone();
    quiet.description = "".into();
    let empty = archive(
        &quiet,
        &Archive {
            kind: &ArchiveKind::Home,
            url: &quiet.home,
            page: 1,
            pages: 1,
            items: &[],
        },
    );
    assert_eq!(
        text(&empty),
        "<title>Example &amp; Co</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/\">\n<!--fp:/head--><p>Nothing has been published here yet.</p>\n"
    );
}

#[test]
fn term_author_and_date_archives_name_themselves() {
    let site = site();
    let items = [post(1, "x")];
    let render = |kind: &ArchiveKind, url: &str, page: u32| {
        String::from_utf8(
            archive(
                &site,
                &Archive {
                    kind,
                    url,
                    page,
                    pages: 2,
                    items: &items,
                },
            )
            .bytes,
        )
        .expect("utf-8")
    };
    let news = term("News", "news", "category");
    let html = render(&ArchiveKind::Category(news.clone()), &news.link, 2);
    assert!(html.starts_with("<title>News \u{2013} Page 2 \u{2013} Example &amp; Co</title>\n<link rel=\"canonical\" href=\"https://example.com/blog/category/news/page/2/\">\n<link rel=\"alternate\" type=\"application/rss+xml\" title=\"Example &amp; Co \u{bb} News Category Feed\" href=\"https://example.com/blog/category/news/feed/\">\n<!--fp:/head--><header class=\"archive\"><h1>Category: News</h1></header>"));
    let tips = term("Tips", "tips", "tag");
    assert!(
        render(&ArchiveKind::Tag(tips.clone()), &tips.link, 1)
            .contains("title=\"Example &amp; Co \u{bb} Tips Tag Feed\"")
    );
    assert!(render(&ArchiveKind::Tag(tips.clone()), &tips.link, 1).contains("<h1>Tag: Tips</h1>"));
    let author = post(1, "").author;
    let html = render(&ArchiveKind::Author(author.clone()), &author.link, 1);
    assert!(html.starts_with("<title>Ann Writer \u{2013} Example &amp; Co</title>"));
    assert!(html.contains("<h1>Author: Ann Writer</h1>"));
    let year = ArchiveKind::Date {
        year: 2026,
        month: None,
        day: None,
    };
    assert!(render(&year, "https://example.com/blog/2026/", 1).contains("<h1>Year: 2026</h1>"));
    let month = ArchiveKind::Date {
        year: 2026,
        month: Some(11),
        day: None,
    };
    assert!(
        render(&month, "https://example.com/blog/2026/11/", 1)
            .contains("<h1>Month: November 2026</h1>")
    );
    let day = ArchiveKind::Date {
        year: 2026,
        month: Some(11),
        day: Some(27),
    };
    let html = render(&day, "https://example.com/blog/2026/11/27/", 1);
    assert!(html.starts_with("<title>November 27, 2026 \u{2013} Example &amp; Co</title>"));
    assert!(html.contains("<h1>Day: November 27, 2026</h1>"));
}

#[test]
fn the_not_found_page_is_a_noindex_fragment() {
    let out = not_found(&site());
    assert_eq!(
        text(&out),
        "<title>Page not found \u{2013} Example &amp; Co</title>\n<meta name=\"robots\" content=\"noindex\">\n<!--fp:/head--><header class=\"archive\"><h1>Page not found</h1></header>\n<p>Nothing was found at this address. Try the <a href=\"https://example.com/blog/\">home page</a>.</p>\n"
    );
    assert!(assemble(&shell(&site()).bytes, &out.bytes).is_ok());
}

#[test]
fn excerpts_are_cut_at_fifty_five_words() {
    let long: Vec<String> = (1..=60).map(|n| format!("w{n}")).collect();
    let post = post(1, &format!("<p>[[92130]]</p><p>{}</p>", long.join(" ")));
    let cut = excerpt(&post);
    assert!(cut.starts_with("w1 w2 "));
    assert!(cut.ends_with(" w55 [\u{2026}]"));
}

#[test]
fn feeds_are_rss_two_with_cdata_content() {
    let site = site();
    let mut first = post(1, "<p>One ]]> two</p>\n<p>[[92130]]</p>");
    first.modified_gmt = stamp("2026-11-29 01:02:03");
    let items = [first, post(2, "Two")];
    let channel = Channel {
        name: Some("News"),
        link: "https://example.com/blog/category/news/",
        self_url: "https://example.com/blog/category/news/feed/",
        items: &items,
    };
    let out = feed(&site, &channel, &mut NoCards).expect("render");
    assert_eq!(out.unresolved, vec![Marker::Zip("92130".into())]);
    assert_eq!(
        out.rendered.content_type,
        "application/rss+xml; charset=UTF-8"
    );
    let xml = text(&out.rendered);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\""));
    assert!(xml.contains("<channel>\n<title>Example &amp; Co \u{bb} News</title>\n<atom:link href=\"https://example.com/blog/category/news/feed/\" rel=\"self\" type=\"application/rss+xml\" />\n<link>https://example.com/blog/category/news/</link>\n<description>Notes &lt;daily&gt;</description>\n<lastBuildDate>Sun, 29 Nov 2026 01:02:03 +0000</lastBuildDate>\n<language>en-US</language>\n<item>\n<title>Post &quot;1&quot;</title>\n<link>https://example.com/blog/post-1/</link>\n<dc:creator><![CDATA[Ann Writer]]></dc:creator>\n<pubDate>Fri, 27 Nov 2026 18:00:00 +0000</pubDate>\n<category><![CDATA[News]]></category>\n<category><![CDATA[Tips & Tricks]]></category>\n<guid isPermaLink=\"false\">https://example.com/blog/?p=1</guid>\n<description><![CDATA[One ]]]]><![CDATA[> two]]></description>\n<content:encoded><![CDATA[<p>One ]]]]><![CDATA[> two</p>\n]]></content:encoded>\n</item>"));
    assert!(xml.ends_with("</item>\n</channel>\n</rss>\n"));
    let empty = Channel {
        name: None,
        link: "https://example.com/blog/",
        self_url: "https://example.com/blog/feed/",
        items: &[],
    };
    let xml = feed(&site, &empty, &mut NoCards)
        .expect("render")
        .rendered
        .bytes;
    let xml = String::from_utf8(xml).expect("utf-8");
    assert!(xml.contains("<title>Example &amp; Co</title>"));
    assert!(!xml.contains("lastBuildDate"));
}

#[test]
fn sitemaps_follow_the_core_paths_and_limits() {
    let site = site();
    let urls = SitemapUrls {
        posts: (0..=URLS_PER_SITEMAP)
            .map(|n| format!("https://example.com/blog/p{n}/").into())
            .collect(),
        pages: vec!["https://example.com/blog/about/".into()],
        categories: vec!["https://example.com/blog/category/a&b/".into()],
        tags: Vec::new(),
        users: vec!["https://example.com/blog/author/ann/".into()],
    };
    let files = sitemaps(&site, &urls);
    let paths: Vec<&str> = files.iter().map(|f| &*f.path).collect();
    assert_eq!(
        paths,
        [
            "wp-sitemap.xml",
            "wp-sitemap-posts-post-1.xml",
            "wp-sitemap-posts-post-2.xml",
            "wp-sitemap-posts-page-1.xml",
            "wp-sitemap-taxonomies-category-1.xml",
            "wp-sitemap-users-1.xml",
        ]
    );
    assert_eq!(
        files[0].rendered.content_type,
        "application/xml; charset=UTF-8"
    );
    assert_eq!(
        text(&files[0].rendered),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"><sitemap><loc>https://example.com/blog/wp-sitemap-posts-post-1.xml</loc></sitemap><sitemap><loc>https://example.com/blog/wp-sitemap-posts-post-2.xml</loc></sitemap><sitemap><loc>https://example.com/blog/wp-sitemap-posts-page-1.xml</loc></sitemap><sitemap><loc>https://example.com/blog/wp-sitemap-taxonomies-category-1.xml</loc></sitemap><sitemap><loc>https://example.com/blog/wp-sitemap-users-1.xml</loc></sitemap></sitemapindex>\n"
    );
    assert_eq!(
        text(&files[1].rendered).matches("<url>").count(),
        URLS_PER_SITEMAP
    );
    assert_eq!(
        text(&files[2].rendered),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"><url><loc>https://example.com/blog/p2000/</loc></url></urlset>\n"
    );
    assert!(text(&files[3].rendered).contains("<url><loc>https://example.com/blog/</loc></url><url><loc>https://example.com/blog/about/</loc></url>"));
    assert!(text(&files[4].rendered).contains("category/a&amp;b/"));
}

#[test]
fn robots_names_the_sitemap_index() {
    let out = robots(&site());
    assert_eq!(out.content_type, "text/plain; charset=UTF-8");
    assert_eq!(
        text(&out),
        "User-agent: *\nDisallow: /blog/admin/\n\nSitemap: https://example.com/blog/wp-sitemap.xml\n"
    );
}
