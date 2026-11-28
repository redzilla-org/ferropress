//! Pull reader over a WXR 1.2 export: one [`Record`] per call, in document order.

use std::fmt;
use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::ParseError;
use crate::record::{self, Record};
use crate::tree::Node;

/// Streams records out of an export file.
///
/// Only the record being read is held in memory. Channel children that carry nothing to
/// import (title, generator, language, …) are read and dropped.
pub struct Wxr<R> {
    reader: Reader<R>,
    buf: Vec<u8>,
    scratch: Vec<u8>,
    in_channel: bool,
}

impl<R: BufRead> Wxr<R> {
    /// A reader over `input`.
    pub fn new(input: R) -> Self {
        Self {
            reader: Reader::from_reader(input),
            buf: Vec::new(),
            scratch: Vec::new(),
            in_channel: false,
        }
    }

    /// The next record, or `None` at the end of the document.
    ///
    /// # Errors
    ///
    /// [`ParseError`] when the XML is malformed, truncated, or a numeric field is not a number.
    pub fn next_record(&mut self) -> Result<Option<Record>, ParseError> {
        loop {
            self.buf.clear();
            let node = match self.reader.read_event_into(&mut self.buf)? {
                Event::Eof => return Ok(None),
                Event::Start(start) if start.name().into_inner() == "channel" => {
                    self.in_channel = true;
                    continue;
                }
                Event::End(end) if end.name().into_inner() == "channel" => {
                    self.in_channel = false;
                    continue;
                }
                // Channel children are whole records; everything above them is a wrapper.
                Event::Start(start) if self.in_channel => Node::open(&start)?,
                _ => continue,
            };
            let node = node.fill(&mut self.reader, &mut self.scratch)?;
            let record = match node.name() {
                "item" => Record::Item(Box::new(record::item(&node)?)),
                "wp:author" => Record::Author(record::author(&node)?),
                "wp:category" | "wp:tag" | "wp:term" => Record::Term(record::term(&node)?),
                "wp:base_site_url" => Record::Site(node.into_text().trim().to_owned()),
                _ => continue,
            };
            return Ok(Some(record));
        }
    }
}

impl<R> fmt::Debug for Wxr<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wxr")
            .field("in_channel", &self.in_channel)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Wxr;
    use crate::error::ParseError;
    use crate::record::{Item, Record};

    const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <rss version=\"2.0\" xmlns:wp=\"http://wordpress.org/export/1.2/\"><channel>\
        <title>Example</title><wp:wxr_version>1.2</wp:wxr_version>";

    fn records(xml: &str) -> Result<Vec<Record>, ParseError> {
        let mut wxr = Wxr::new(xml.as_bytes());
        let mut out = Vec::new();
        while let Some(record) = wxr.next_record()? {
            out.push(record);
        }
        Ok(out)
    }

    fn only_item(xml: &str) -> Option<Item> {
        match records(xml).ok()?.pop()? {
            Record::Item(item) => Some(*item),
            _ => None,
        }
    }

    #[test]
    fn channel_children_become_records_in_order() {
        let xml = format!(
            "{HEAD}<wp:base_site_url> https://old.example.com </wp:base_site_url>\
             <wp:author><wp:author_id>3</wp:author_id><wp:author_login><![CDATA[ada]]></wp:author_login>\
             <wp:author_email>ada@example.com</wp:author_email><wp:author_display_name>Ada</wp:author_display_name>\
             <wp:author_first_name>A</wp:author_first_name><wp:author_last_name>L</wp:author_last_name></wp:author>\
             <wp:category><wp:term_id>5</wp:term_id><wp:category_nicename>news</wp:category_nicename>\
             <wp:category_parent>top</wp:category_parent><wp:cat_name><![CDATA[News &amp; more]]></wp:cat_name>\
             <wp:category_description>d</wp:category_description></wp:category>\
             <wp:tag><wp:term_id>6</wp:term_id><wp:tag_slug>t</wp:tag_slug><wp:tag_name>T</wp:tag_name></wp:tag>\
             <wp:term><wp:term_id>7</wp:term_id><wp:term_taxonomy>genre</wp:term_taxonomy><wp:term_slug>g</wp:term_slug>\
             <wp:term_parent></wp:term_parent><wp:term_name>G</wp:term_name>\
             <wp:termmeta><wp:meta_key>color</wp:meta_key><wp:meta_value>red</wp:meta_value></wp:termmeta></wp:term>\
             <generator>x</generator></channel></rss>"
        );
        let parsed = records(&xml);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        assert_eq!(parsed.len(), 5);
        assert_eq!(
            parsed[0],
            Record::Site("https://old.example.com".to_owned())
        );
        assert!(
            matches!(&parsed[1], Record::Author(a) if a.id == 3 && a.login == "ada"
            && a.email == "ada@example.com" && a.display_name == "Ada"
            && a.first_name == "A" && a.last_name == "L")
        );
        assert!(
            matches!(&parsed[2], Record::Term(t) if t.taxonomy == "category" && t.slug == "news"
            && t.parent == "top" && t.name == "News &amp; more" && t.description == "d" && t.id == 5)
        );
        assert!(
            matches!(&parsed[3], Record::Term(t) if t.taxonomy == "post_tag" && t.slug == "t"
            && t.parent.is_empty() && t.name == "T")
        );
        assert!(
            matches!(&parsed[4], Record::Term(t) if t.taxonomy == "genre" && t.slug == "g"
            && t.meta.len() == 1 && t.meta[0].key == "color" && t.meta[0].value == "red")
        );
        assert_eq!(parsed[0].weight(), (0, 0));
        assert_eq!(parsed[1].weight().0, 1);
        assert_eq!(parsed[4].weight().0, 3);
    }

    #[test]
    fn an_item_keeps_every_field_meta_link_and_comment() {
        let xml = format!(
            "{HEAD}<item><title>Hello &amp; welcome</title><guid isPermaLink=\"false\">g-1</guid>\
             <dc:creator>ada</dc:creator><content:encoded><![CDATA[<p>Hi &nbsp;</p>]]></content:encoded>\
             <excerpt:encoded>ex</excerpt:encoded><wp:post_id> 12 </wp:post_id>\
             <wp:post_date>2026-01-02 03:04:05</wp:post_date><wp:post_date_gmt>0000-00-00 00:00:00</wp:post_date_gmt>\
             <wp:comment_status>open</wp:comment_status><wp:ping_status>closed</wp:ping_status>\
             <wp:post_name>hello</wp:post_name><wp:status>draft</wp:status><wp:post_parent>4</wp:post_parent>\
             <wp:menu_order>2</wp:menu_order><wp:post_type>post</wp:post_type><wp:post_password>pw</wp:post_password>\
             <wp:post_mime_type></wp:post_mime_type>\
             <category domain=\"category\" nicename=\"news\"><![CDATA[News]]></category>\
             <category>legacy</category>\
             <wp:postmeta><wp:meta_key>k</wp:meta_key><wp:meta_value><![CDATA[v]]></wp:meta_value></wp:postmeta>\
             <wp:comment><wp:comment_id>9</wp:comment_id><wp:comment_author>Bo</wp:comment_author>\
             <wp:comment_author_email>bo@example.com</wp:comment_author_email><wp:comment_author_url>u</wp:comment_author_url>\
             <wp:comment_author_IP>127.0.0.1</wp:comment_author_IP><wp:comment_date>d</wp:comment_date>\
             <wp:comment_date_gmt>dg</wp:comment_date_gmt><wp:comment_content>c</wp:comment_content>\
             <wp:comment_approved>1</wp:comment_approved><wp:comment_type>comment</wp:comment_type>\
             <wp:comment_parent>8</wp:comment_parent><wp:comment_user_id>3</wp:comment_user_id>\
             <wp:commentmeta><wp:meta_key>ck</wp:meta_key><wp:meta_value>cv</wp:meta_value></wp:commentmeta>\
             </wp:comment></item></channel></rss>"
        );
        let item = only_item(&xml);
        assert!(item.is_some());
        let Some(item) = item else { return };
        assert_eq!(item.title, "Hello & welcome");
        assert_eq!(item.guid, "g-1");
        assert_eq!(item.creator, "ada");
        assert_eq!(item.content, "<p>Hi &nbsp;</p>");
        assert_eq!(item.excerpt, "ex");
        assert_eq!(item.id, 12);
        assert_eq!(item.date, "2026-01-02 03:04:05");
        assert_eq!(item.modified, item.date);
        assert_eq!(item.modified_gmt, "0000-00-00 00:00:00");
        assert_eq!(
            (item.comment_status.as_str(), item.ping_status.as_str()),
            ("open", "closed")
        );
        assert_eq!(
            (item.name.as_str(), item.status.as_str(), item.kind.as_str()),
            ("hello", "draft", "post")
        );
        assert_eq!((item.parent, item.menu_order), (4, 2));
        assert_eq!(item.password, "pw");
        assert_eq!(item.terms.len(), 1);
        assert_eq!(
            (
                item.terms[0].taxonomy.as_str(),
                item.terms[0].slug.as_str(),
                item.terms[0].name.as_str()
            ),
            ("category", "news", "News")
        );
        assert_eq!(
            (item.meta[0].key.as_str(), item.meta[0].value.as_str()),
            ("k", "v")
        );
        let comment = &item.comments[0];
        assert_eq!((comment.id, comment.parent, comment.user_id), (9, 8, 3));
        assert_eq!(comment.author, "Bo");
        assert_eq!(comment.author_email, "bo@example.com");
        assert_eq!(comment.author_url, "u");
        assert_eq!(comment.author_ip, "127.0.0.1");
        assert_eq!(
            (comment.date.as_str(), comment.date_gmt.as_str()),
            ("d", "dg")
        );
        assert_eq!(
            (
                comment.content.as_str(),
                comment.approved.as_str(),
                comment.kind.as_str()
            ),
            ("c", "1", "comment")
        );
        assert_eq!(comment.meta[0].key, "ck");
        let weight = Record::Item(Box::new(item)).weight();
        assert_eq!(weight.0, 2 + 1 + 3 + 2);
    }

    #[test]
    fn explicit_modified_dates_win() {
        let xml = format!(
            "{HEAD}<item><guid>g</guid><wp:post_date>a</wp:post_date><wp:post_modified>b</wp:post_modified>\
             <wp:post_modified_gmt>c</wp:post_modified_gmt></item></channel></rss>"
        );
        let item = only_item(&xml);
        assert!(item.is_some_and(|item| item.modified == "b" && item.modified_gmt == "c"));
    }

    #[test]
    fn bad_numbers_and_truncation_fail() {
        let bad = format!("{HEAD}<item><wp:post_id>x</wp:post_id></item></channel></rss>");
        assert!(matches!(
            records(&bad),
            Err(ParseError::Number {
                field: "wp:post_id",
                ..
            })
        ));
        let bad_comment = format!(
            "{HEAD}<item><wp:comment><wp:comment_id>-1</wp:comment_id></wp:comment></item></channel></rss>"
        );
        assert!(matches!(
            records(&bad_comment),
            Err(ParseError::Number { .. })
        ));
        let bad_author = format!("{HEAD}<wp:author><wp:author_id>?</wp:author_id></wp:author>");
        assert!(records(&bad_author).is_err());
        let bad_term = format!("{HEAD}<wp:tag><wp:term_id>?</wp:term_id></wp:tag>");
        assert!(records(&bad_term).is_err());
        let truncated = format!("{HEAD}<item><title>x</title>");
        assert!(matches!(records(&truncated), Err(ParseError::Truncated(_))));
        assert!(records("<rss></rss>").is_ok_and(|all| all.is_empty()));
        assert!(format!("{:?}", Wxr::new(&b""[..])).contains("Wxr"));
    }
}
