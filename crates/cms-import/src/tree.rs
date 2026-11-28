//! One channel child (an item, author or term) read into a small owned tree.
//!
//! Only one record's subtree is held at a time, so memory is bounded by the largest record,
//! never by the export file.

use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::ParseError;
use crate::text::{push_entity, unescape};

/// WXR records nest at most four levels (item > comment > commentmeta > key); this leaves room
/// for exporters that wrap values while still rejecting runaway input.
const MAX_DEPTH: usize = 16;

/// An element with its attributes, its concatenated text and CDATA, and its child elements.
#[derive(Debug, Default)]
pub(crate) struct Node {
    name: Box<str>,
    attrs: Vec<(Box<str>, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    /// A node named and attributed like `start`, with no text or children yet.
    pub(crate) fn open(start: &BytesStart<'_>) -> Result<Self, ParseError> {
        let mut attrs = Vec::new();
        for attr in start.attributes() {
            let attr = attr?;
            attrs.push((Box::from(attr.key.into_inner()), unescape(&attr.value)));
        }
        Ok(Self {
            name: Box::from(start.name().into_inner()),
            attrs,
            text: String::new(),
            children: Vec::new(),
        })
    }

    /// The qualified element name as written (`wp:post_id`).
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The text of this element, entities decoded and CDATA kept byte for byte.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The text of this element, taking ownership.
    pub(crate) fn into_text(self) -> String {
        self.text
    }

    /// The value of attribute `name`, if present.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.as_ref() == name)
            .map(|(_, value)| value.as_str())
    }

    /// Direct children named `name`, in document order.
    pub(crate) fn children<'a, 'n>(
        &'a self,
        name: &'n str,
    ) -> impl Iterator<Item = &'a Node> + use<'a, 'n> {
        self.children
            .iter()
            .filter(move |child| child.name() == name)
    }

    /// The text of the first direct child named `name`, or empty when there is none.
    pub(crate) fn field(&self, name: &str) -> &str {
        self.children
            .iter()
            .find(|child| child.name() == name)
            .map_or("", Node::text)
    }

    /// Reads events after `self`'s start tag up to its matching end tag.
    pub(crate) fn fill<R: BufRead>(
        self,
        reader: &mut Reader<R>,
        buf: &mut Vec<u8>,
    ) -> Result<Self, ParseError> {
        let mut stack = vec![self];
        loop {
            buf.clear();
            let event = reader.read_event_into(buf)?;
            let Some(top) = stack.last_mut() else {
                return Err(ParseError::Truncated(Box::from("")));
            };
            match event {
                Event::Start(start) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(ParseError::TooDeep(MAX_DEPTH));
                    }
                    let child = Node::open(&start)?;
                    stack.push(child);
                }
                Event::Empty(start) => top.children.push(Node::open(&start)?),
                Event::Text(text) => top.text.push_str(&text),
                Event::CData(data) => top.text.push_str(&data),
                Event::GeneralRef(reference) => push_entity(&mut top.text, &reference),
                Event::End(_) => {
                    // The reader checks end names, so the closed element is the top one.
                    let Some(done) = stack.pop() else {
                        return Err(ParseError::Truncated(Box::from("")));
                    };
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(done),
                        None => return Ok(done),
                    }
                }
                Event::Eof => return Err(ParseError::Truncated(top.name.clone())),
                Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DEPTH, Node};
    use crate::error::ParseError;
    use quick_xml::Reader;
    use quick_xml::events::Event;

    fn read(xml: &str) -> Result<Node, ParseError> {
        let mut reader = Reader::from_reader(xml.as_bytes());
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if let Event::Start(start) = reader.read_event_into(&mut buf)? {
                let node = Node::open(&start)?;
                let mut scratch = Vec::new();
                return node.fill(&mut reader, &mut scratch);
            }
        }
    }

    #[test]
    fn a_subtree_keeps_text_cdata_attributes_and_children() {
        let node = read(
            "<item a=\"x &amp; y\"><title><![CDATA[A <b>&amp;</b>]]></title>\
             <p>&lt;&nbsp;&#65;</p><empty k=\"v\"/><!-- c --><?pi x?>\
             <wp:meta><wp:meta_key>k</wp:meta_key></wp:meta><p>second</p></item>",
        );
        assert!(node.is_ok());
        if let Ok(node) = node {
            assert_eq!(node.name(), "item");
            assert_eq!(node.attr("a"), Some("x & y"));
            assert_eq!(node.attr("missing"), None);
            assert_eq!(node.field("title"), "A <b>&amp;</b>");
            assert_eq!(node.field("p"), "<&nbsp;A");
            assert_eq!(node.children("p").count(), 2);
            assert_eq!(node.field("absent"), "");
            let empty = node.children("empty").next();
            assert!(empty.is_some_and(|empty| empty.attr("k") == Some("v")));
            let meta = node.children("wp:meta").next();
            assert!(meta.is_some_and(|meta| meta.field("wp:meta_key") == "k"));
            assert_eq!(node.text(), "");
        }
    }

    #[test]
    fn truncated_deep_and_malformed_input_is_rejected() {
        assert!(matches!(
            read("<item><title>x</title>"),
            Err(ParseError::Truncated(name)) if name.as_ref() == "item"
        ));
        let deep = format!(
            "{}{}",
            "<a>".repeat(MAX_DEPTH + 1),
            "</a>".repeat(MAX_DEPTH + 1)
        );
        assert!(matches!(read(&deep), Err(ParseError::TooDeep(_))));
        assert!(matches!(
            read("<item><a></b></item>"),
            Err(ParseError::Xml(_))
        ));
        assert!(matches!(
            read("<item><a x=\"1\" x=\"2\"/></item>"),
            Err(ParseError::Attribute(_))
        ));
    }

    #[test]
    fn into_text_takes_the_text() {
        let node = read("<t>hello</t>");
        assert!(node.is_ok_and(|node| node.into_text() == "hello"));
    }
}
