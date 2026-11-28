//! Listing card markers in post content and the cards that replace them.
//!
//! A marker is a bare token in double brackets: five digits is a ZIP card, any other token of
//! 6 to 20 ASCII letters and digits with at least one digit is a listing card. Letters-only text
//! such as `[[gallery]]` is not a marker. A marker alone in a paragraph replaces the paragraph,
//! so a card never lands inside `<p>`. Card data comes from a [`CardSource`] port; the HTTP
//! adapter for the listing card endpoint lives outside this crate.

use std::fmt::Write as _;

use crate::escape::push_escaped;

/// One card marker found in content.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Marker {
    /// `[[<5 digits>]]`: area figures and current listings for a ZIP code.
    Zip(Box<str>),
    /// `[[<listing id>]]`: one listing, id upper-cased.
    Listing(Box<str>),
}

impl Marker {
    /// Classifies the token between the brackets, or `None` when it is not a marker.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let shaped = (5..=20).contains(&token.len())
            && token.bytes().all(|b| b.is_ascii_alphanumeric())
            && token.bytes().any(|b| b.is_ascii_digit());
        if !shaped {
            return None;
        }
        if token.len() == 5 && token.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Self::Zip(token.into()));
        }
        Some(Self::Listing(token.to_ascii_uppercase().into()))
    }
}

/// Figures for one ZIP code, ready to print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipCard {
    /// The ZIP code.
    pub zip: Box<str>,
    /// Closed sales over the chosen period, or `None` when too few sales to report.
    pub sold: Option<SoldFigures>,
    /// Homes for sale now.
    pub active: u32,
    /// The day `active` was counted, as display text.
    pub as_of: Box<str>,
    /// Median list price of the homes for sale, if enough to report.
    pub median_list: Option<u64>,
    /// Up to a handful of current listings.
    pub tiles: Vec<Tile>,
    /// Search URL for all homes for sale in the ZIP.
    pub search_url: Box<str>,
    /// Source and attribution line printed under the card.
    pub credit: Box<str>,
}

/// Closed-sale figures for a period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoldFigures {
    /// The period in words, such as `3 months to October 2026`.
    pub period: Box<str>,
    /// Closed sales.
    pub sales: u32,
    /// Median sale price, if enough sales.
    pub median_price: Option<u64>,
    /// Median price per square foot, if enough sales.
    pub median_per_sqft: Option<u64>,
}

/// One current listing inside a ZIP card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Listing page URL.
    pub url: Box<str>,
    /// Photo URL, if any.
    pub photo: Option<Box<str>>,
    /// List price, if known.
    pub price: Option<u64>,
    /// Facts line parts.
    pub facts: Facts,
    /// Locality, empty when withheld.
    pub place: Box<str>,
}

/// Bedroom, bathroom and size facts; zero or empty means unknown and is not printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Bedrooms.
    pub beds: u32,
    /// Bathrooms as given by the source.
    pub baths: Box<str>,
    /// Living area in square feet.
    pub sqft: u64,
}

/// One sold listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingCard {
    /// Listing page URL.
    pub url: Box<str>,
    /// Photo URL, if any.
    pub photo: Option<Box<str>>,
    /// Street address, empty when withheld.
    pub address: Box<str>,
    /// Locality line, empty when withheld.
    pub place: Box<str>,
    /// Sale price.
    pub price: u64,
    /// Sale date as display text, empty if unknown.
    pub date: Box<str>,
    /// Facts.
    pub facts: Facts,
    /// Source and attribution line.
    pub credit: Box<str>,
}

/// Card data for one marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Card {
    /// For a [`Marker::Zip`].
    Zip(ZipCard),
    /// For a [`Marker::Listing`].
    Listing(ListingCard),
}

/// The card source could not be asked; the render fails so the projector retries.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("listing cards unavailable: {0}")]
pub struct CardError(pub Box<str>);

/// Resolves markers to card data (the listing card endpoint in production).
pub trait CardSource {
    /// The card for `marker`, or `None` when there is nothing to show (unknown, withheld or
    /// outside the source's market).
    ///
    /// # Errors
    ///
    /// [`CardError`] when the source cannot be reached or answers malformed data.
    fn card(&mut self, marker: &Marker) -> Result<Option<Card>, CardError>;
}

/// A source with no cards: every marker renders as nothing and is reported.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCards;

impl CardSource for NoCards {
    fn card(&mut self, _marker: &Marker) -> Result<Option<Card>, CardError> {
        Ok(None)
    }
}

// One marker occurrence: the byte range it replaces and what it is.
struct Found {
    start: usize,
    end: usize,
    marker: Marker,
}

fn markers(html: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(open) = html[from..].find("[[").map(|at| from + at) {
        let Some(close) = html[open + 2..].find("]]").map(|at| open + 2 + at) else {
            break;
        };
        let Some(marker) = Marker::parse(&html[open + 2..close]) else {
            from = open + 2;
            continue;
        };
        let (mut start, mut end) = (open, close + 2);
        // A marker alone in its paragraph takes the paragraph with it.
        let before = html[..start].trim_end();
        let after = html[end..].trim_start();
        if before.ends_with("<p>") && after.starts_with("</p>") {
            start = before.len() - 3;
            end = html.len() - after.len() + 4;
        }
        found.push(Found { start, end, marker });
        from = end;
    }
    found
}

/// Content with every marker removed, for excerpts and plain-text uses.
#[must_use]
pub fn strip_markers(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    for found in markers(html) {
        out.push_str(&html[at..found.start]);
        at = found.end;
    }
    out.push_str(&html[at..]);
    out
}

/// Formatted content with its markers replaced by cards, plus the markers that had no card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithCards {
    /// The HTML.
    pub html: String,
    /// Markers the source had no card for; they render as nothing.
    pub unresolved: Vec<Marker>,
}

/// Replaces each marker in `html` with its card from `source`.
///
/// # Errors
///
/// [`CardError`] from the source.
pub fn render_markers<C: CardSource + ?Sized>(
    html: &str,
    source: &mut C,
) -> Result<WithCards, CardError> {
    let mut out = String::with_capacity(html.len());
    let mut unresolved = Vec::new();
    let mut at = 0;
    for found in markers(html) {
        out.push_str(&html[at..found.start]);
        at = found.end;
        match source.card(&found.marker)? {
            Some(card) => push_card(&mut out, &card),
            None => unresolved.push(found.marker),
        }
    }
    out.push_str(&html[at..]);
    Ok(WithCards {
        html: out,
        unresolved,
    })
}

/// Appends one card's HTML; every field is escaped.
pub fn push_card(out: &mut String, card: &Card) {
    match card {
        Card::Zip(zip) => push_zip(out, zip),
        Card::Listing(listing) => push_listing(out, listing),
    }
}

fn push_zip(out: &mut String, card: &ZipCard) {
    out.push_str("<div class=\"fp-card fp-card-zip\">");
    if let Some(sold) = &card.sold {
        out.push_str("<table><caption>");
        push_escaped(
            out,
            &format!("ZIP {} home sales: {}", card.zip, sold.period),
        );
        out.push_str("</caption><tbody>");
        row(out, "Closed sales", &sold.sales.to_string());
        row(out, "Median sale price", &money_or_few(sold.median_price));
        row(
            out,
            "Median price per sq ft",
            &money_or_few(sold.median_per_sqft),
        );
        out.push_str("</tbody></table>");
    } else {
        out.push_str("<p>");
        push_escaped(
            out,
            &format!("ZIP {}: too few home sales to report.", card.zip),
        );
        out.push_str("</p>");
    }
    out.push_str("<p>");
    let mut line = format!(
        "ZIP {} homes for sale on {}: {}",
        card.zip, card.as_of, card.active
    );
    if let Some(median) = card.median_list {
        let _ = write!(line, ". Median list price: {}", money(median));
    }
    line.push('.');
    push_escaped(out, &line);
    out.push_str("</p>");
    if !card.tiles.is_empty() {
        out.push_str("<div class=\"fp-card-tiles\">");
        for tile in &card.tiles {
            push_tile(out, &card.zip, tile);
        }
        out.push_str("</div>");
    }
    out.push_str("<p><a href=\"");
    push_escaped(out, &card.search_url);
    out.push_str("\">");
    push_escaped(out, &format!("See all homes for sale in {}", card.zip));
    out.push_str("</a></p>");
    push_credit(out, &card.credit);
    out.push_str("</div>");
}

fn push_tile(out: &mut String, zip: &str, tile: &Tile) {
    out.push_str("<a class=\"fp-card-tile\" href=\"");
    push_escaped(out, &tile.url);
    out.push_str("\">");
    if let Some(photo) = &tile.photo {
        push_image(out, photo, &format!("Home for sale in ZIP {zip}"));
    }
    if let Some(price) = tile.price {
        span(out, &money(price));
    }
    if let Some(facts) = facts_line(&tile.facts) {
        span(out, &facts);
    }
    if !tile.place.is_empty() {
        span(out, &tile.place);
    }
    out.push_str("</a>");
}

fn push_listing(out: &mut String, card: &ListingCard) {
    let address = if card.address.is_empty() {
        "Address not disclosed"
    } else {
        &card.address
    };
    out.push_str("<div class=\"fp-card fp-card-listing\">");
    if let Some(photo) = &card.photo {
        out.push_str("<a href=\"");
        push_escaped(out, &card.url);
        out.push_str("\">");
        push_image(out, photo, &format!("Sold: {address}"));
        out.push_str("</a>");
    }
    out.push_str(
        "<div><span class=\"fp-card-flag\">SOLD</span><p class=\"fp-card-address\"><a href=\"",
    );
    push_escaped(out, &card.url);
    out.push_str("\">");
    push_escaped(out, address);
    out.push_str("</a></p>");
    if !card.place.is_empty() {
        paragraph(out, &card.place);
    }
    out.push_str("<p class=\"fp-card-price\">");
    push_escaped(out, &money(card.price));
    out.push_str("</p>");
    if let Some(facts) = facts_line(&card.facts) {
        paragraph(out, &facts);
    }
    if !card.date.is_empty() {
        paragraph(out, &format!("Sold {}", card.date));
    }
    push_credit(out, &card.credit);
    out.push_str("</div></div>");
}

fn push_image(out: &mut String, src: &str, alt: &str) {
    out.push_str("<img src=\"");
    push_escaped(out, src);
    out.push_str("\" alt=\"");
    push_escaped(out, alt);
    out.push_str("\" loading=\"lazy\">");
}

fn push_credit(out: &mut String, credit: &str) {
    if !credit.is_empty() {
        out.push_str("<p class=\"fp-card-credit\">");
        push_escaped(out, credit);
        out.push_str("</p>");
    }
}

fn row(out: &mut String, name: &str, value: &str) {
    out.push_str("<tr><th scope=\"row\">");
    push_escaped(out, name);
    out.push_str("</th><td>");
    push_escaped(out, value);
    out.push_str("</td></tr>");
}

fn span(out: &mut String, text: &str) {
    out.push_str("<span>");
    push_escaped(out, text);
    out.push_str("</span>");
}

fn paragraph(out: &mut String, text: &str) {
    out.push_str("<p>");
    push_escaped(out, text);
    out.push_str("</p>");
}

fn facts_line(facts: &Facts) -> Option<String> {
    let mut parts = Vec::with_capacity(3);
    if facts.beds > 0 {
        parts.push(format!("{} bed", facts.beds));
    }
    if !facts.baths.is_empty() {
        parts.push(format!("{} bath", facts.baths));
    }
    if facts.sqft > 0 {
        parts.push(format!("{} sq ft", grouped(facts.sqft)));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

fn money_or_few(value: Option<u64>) -> String {
    value.map_or_else(|| "Too few sales".to_owned(), money)
}

fn money(value: u64) -> String {
    format!("${}", grouped(value))
}

// Thousands separated by commas, as US prices are printed.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Card, CardError, CardSource, Facts, ListingCard, Marker, NoCards, SoldFigures, Tile,
        ZipCard, grouped, render_markers, strip_markers,
    };

    struct Fixed(Option<Card>, bool);

    impl CardSource for Fixed {
        fn card(&mut self, _marker: &Marker) -> Result<Option<Card>, CardError> {
            if self.1 {
                return Err(CardError("timeout".into()));
            }
            Ok(self.0.clone())
        }
    }

    fn listing() -> ListingCard {
        ListingCard {
            url: "https://listings.example.com/a?b=1&c=2".into(),
            photo: Some("https://img.example.com/1.jpg".into()),
            address: "1 Main <St>".into(),
            place: "Town, ST 00001".into(),
            price: 1_250_000,
            date: "November 2, 2026".into(),
            facts: Facts {
                beds: 3,
                baths: "2".into(),
                sqft: 1800,
            },
            credit: "Source: Example".into(),
        }
    }

    #[test]
    fn markers_classify_tokens() {
        assert_eq!(Marker::parse("92130"), Some(Marker::Zip("92130".into())));
        assert_eq!(
            Marker::parse("ab12345"),
            Some(Marker::Listing("AB12345".into()))
        );
        assert_eq!(
            Marker::parse("AB123"),
            Some(Marker::Listing("AB123".into()))
        );
        assert_eq!(Marker::parse("gallery"), None);
        assert_eq!(Marker::parse("1234"), None);
        assert_eq!(Marker::parse("123456789012345678901"), None);
        assert_eq!(Marker::parse("12-345"), None);
    }

    #[test]
    fn unresolved_markers_vanish_with_their_paragraph_and_are_reported() {
        let html = "<p>a [[92130]] b</p>\n<p> [[x1y2z3]] </p>[[gallery]][[ 1 ]]<p>[[123";
        let out = render_markers(html, &mut NoCards);
        assert_eq!(
            out,
            Ok(super::WithCards {
                html: "<p>a  b</p>\n[[gallery]][[ 1 ]]<p>[[123".to_owned(),
                unresolved: vec![
                    Marker::Zip("92130".into()),
                    Marker::Listing("X1Y2Z3".into())
                ],
            })
        );
        assert_eq!(strip_markers("<p>[[AB12345]]</p>x[[92130]]"), "x");
    }

    #[test]
    fn source_errors_fail_the_render() {
        let out = render_markers("[[92130]]", &mut Fixed(None, true));
        assert_eq!(
            out.map_err(|e| e.to_string()),
            Err("listing cards unavailable: timeout".to_owned())
        );
        assert_eq!(
            render_markers("none", &mut Fixed(None, true)).map(|w| w.html),
            Ok("none".to_owned())
        );
    }

    #[test]
    fn listing_cards_render_escaped() {
        let out = render_markers(
            "<p>[[AB12345]]</p>",
            &mut Fixed(Some(Card::Listing(listing())), false),
        );
        assert_eq!(
            out.map(|w| w.html),
            Ok("<div class=\"fp-card fp-card-listing\"><a href=\"https://listings.example.com/a?b=1&amp;c=2\">\
<img src=\"https://img.example.com/1.jpg\" alt=\"Sold: 1 Main &lt;St&gt;\" loading=\"lazy\"></a>\
<div><span class=\"fp-card-flag\">SOLD</span><p class=\"fp-card-address\"><a href=\"https://listings.example.com/a?b=1&amp;c=2\">1 Main &lt;St&gt;</a></p>\
<p>Town, ST 00001</p><p class=\"fp-card-price\">$1,250,000</p><p>3 bed \u{b7} 2 bath \u{b7} 1,800 sq ft</p>\
<p>Sold November 2, 2026</p><p class=\"fp-card-credit\">Source: Example</p></div></div>"
                .to_owned())
        );
        let bare = ListingCard {
            photo: None,
            address: "".into(),
            place: "".into(),
            date: "".into(),
            facts: Facts::default(),
            credit: "".into(),
            ..listing()
        };
        let out = render_markers("[[AB12345]]", &mut Fixed(Some(Card::Listing(bare)), false));
        assert_eq!(
            out.map(|w| w.html),
            Ok("<div class=\"fp-card fp-card-listing\"><div><span class=\"fp-card-flag\">SOLD</span>\
<p class=\"fp-card-address\"><a href=\"https://listings.example.com/a?b=1&amp;c=2\">Address not disclosed</a></p>\
<p class=\"fp-card-price\">$1,250,000</p></div></div>"
                .to_owned())
        );
    }

    #[test]
    fn zip_cards_render_figures_and_tiles() {
        let card = ZipCard {
            zip: "92130".into(),
            sold: Some(SoldFigures {
                period: "October 2026".into(),
                sales: 12,
                median_price: Some(2_000_000),
                median_per_sqft: None,
            }),
            active: 4,
            as_of: "November 27, 2026".into(),
            median_list: Some(1_999_000),
            tiles: vec![
                Tile {
                    url: "https://listings.example.com/1".into(),
                    photo: Some("https://img.example.com/t.jpg".into()),
                    price: Some(950_000),
                    facts: Facts {
                        beds: 2,
                        baths: "".into(),
                        sqft: 0,
                    },
                    place: "Town".into(),
                },
                Tile {
                    url: "https://listings.example.com/2".into(),
                    photo: None,
                    price: None,
                    facts: Facts::default(),
                    place: "".into(),
                },
            ],
            search_url: "https://listings.example.com/search?zip=92130".into(),
            credit: "Source: Example".into(),
        };
        let out = render_markers(
            "[[92130]]",
            &mut Fixed(Some(Card::Zip(card.clone())), false),
        );
        assert_eq!(
            out.map(|w| w.html),
            Ok("<div class=\"fp-card fp-card-zip\"><table><caption>ZIP 92130 home sales: October 2026</caption><tbody>\
<tr><th scope=\"row\">Closed sales</th><td>12</td></tr><tr><th scope=\"row\">Median sale price</th><td>$2,000,000</td></tr>\
<tr><th scope=\"row\">Median price per sq ft</th><td>Too few sales</td></tr></tbody></table>\
<p>ZIP 92130 homes for sale on November 27, 2026: 4. Median list price: $1,999,000.</p>\
<div class=\"fp-card-tiles\"><a class=\"fp-card-tile\" href=\"https://listings.example.com/1\">\
<img src=\"https://img.example.com/t.jpg\" alt=\"Home for sale in ZIP 92130\" loading=\"lazy\">\
<span>$950,000</span><span>2 bed</span><span>Town</span></a>\
<a class=\"fp-card-tile\" href=\"https://listings.example.com/2\"></a></div>\
<p><a href=\"https://listings.example.com/search?zip=92130\">See all homes for sale in 92130</a></p>\
<p class=\"fp-card-credit\">Source: Example</p></div>"
                .to_owned())
        );
        let quiet = ZipCard {
            sold: None,
            median_list: None,
            tiles: Vec::new(),
            credit: "".into(),
            ..card
        };
        let out = render_markers("[[92130]]", &mut Fixed(Some(Card::Zip(quiet)), false));
        assert_eq!(
            out.map(|w| w.html),
            Ok("<div class=\"fp-card fp-card-zip\"><p>ZIP 92130: too few home sales to report.</p>\
<p>ZIP 92130 homes for sale on November 27, 2026: 4.</p>\
<p><a href=\"https://listings.example.com/search?zip=92130\">See all homes for sale in 92130</a></p></div>"
                .to_owned())
        );
    }

    #[test]
    fn numbers_group_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
