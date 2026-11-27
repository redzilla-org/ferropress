//! Upload file names: the reference's sanitizing, sub-size naming and uniqueness rules.

use crate::Error;

// Characters the reference strips from upload names before anything else.
const SPECIAL: [char; 33] = [
    '?', '[', ']', '/', '\\', '=', '<', '>', ':', ';', ',', '\'', '"', '&', '$', '#', '*', '(',
    ')', '|', '~', '`', '!', '{', '}', '%', '+', '\u{2019}', '\u{ab}', '\u{bb}', '\u{201d}',
    '\u{201c}', '\0',
];

// The reference's default upload extensions; an intermediate extension listed here keeps its
// dot without the trailing underscore the reference adds to unknown ones.
const KNOWN_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "gif", "png", "bmp", "tiff", "tif", "webp", "avif", "ico", "heic",
    "heif", "heics", "heifs", "asf", "asx", "wmv", "wmx", "wm", "avi", "divx", "flv", "mov", "qt",
    "mpeg", "mpg", "mpe", "mp4", "m4v", "ogv", "webm", "mkv", "3gp", "3gpp", "3g2", "3gp2", "txt",
    "asc", "c", "cc", "h", "srt", "csv", "tsv", "ics", "rtx", "css", "htm", "html", "vtt", "dfxp",
    "mp3", "m4a", "m4b", "aac", "ra", "ram", "wav", "x-wav", "ogg", "oga", "flac", "mid", "midi",
    "wma", "wax", "mka", "rtf", "js", "pdf", "class", "tar", "zip", "gz", "gzip", "rar", "7z",
    "psd", "xcf", "doc", "pot", "pps", "ppt", "wri", "xla", "xls", "xlt", "xlw", "mdb", "mpp",
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xlsb", "xltx", "xltm", "xlam", "pptx", "pptm",
    "ppsx", "ppsm", "potx", "potm", "ppam", "sldx", "sldm", "onetoc", "onetoc2", "onetmp",
    "onepkg", "oxps", "xps", "odt", "odp", "ods", "odg", "odc", "odb", "odf", "wp", "wpd", "key",
    "numbers", "pages",
];

// Feed names an attachment slug must not take (the reference treats them as bad slugs).
const FEEDS: [&str; 5] = ["feed", "rdf", "rss", "rss2", "atom"];

/// The reference's upload file-name sanitizer.
///
/// Folds Latin-1 and Latin Extended-A accents, strips the special characters, turns
/// whitespace and dash runs into one dash, collapses repeated dots, trims `.-_` at both ends,
/// names a bare extension `unnamed-file.<ext>` and suffixes unknown intermediate extensions with
/// `_`. Accents outside those two blocks are kept (owner decision on #10).
///
/// # Errors
///
/// [`Error::Name`] when nothing is left.
pub fn sanitize_file_name(name: &str) -> Result<String, Error> {
    // Accents first, then non-breaking spaces, then the special characters, in reference order.
    let folded = remove_accents(name).replace('\u{a0}', " ");
    let stripped: String = folded.chars().filter(|c| !SPECIAL.contains(c)).collect();
    let dashed = stripped.replace("%20", "-").replace('+', "-");
    let dotted = collapse_dots(&dashed);
    let spaced = collapse_space_runs(&dotted);
    let trimmed = spaced.trim_matches(|c| matches!(c, '.' | '-' | '_'));
    if trimmed.is_empty() {
        return Err(Error::Name);
    }
    if !trimmed.contains('.') {
        return Ok(if is_known_extension(trimmed) {
            format!("unnamed-file.{trimmed}")
        } else {
            trimmed.to_owned()
        });
    }
    Ok(mark_intermediate_extensions(trimmed))
}

/// `name` split at its last dot into stem and extension; no dot gives an empty extension.
pub(crate) fn split(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or((name, ""))
}

/// `stem-WxH.ext`, the reference's sub-size file name.
#[must_use]
pub fn sized_name(stem: &str, extension: &str, width: u32, height: u32) -> String {
    format!("{stem}-{width}x{height}.{extension}")
}

/// `stem-scaled.ext`, the reference's big-image file name.
#[must_use]
pub fn scaled_name(stem: &str, extension: &str) -> String {
    format!("{stem}-scaled.{extension}")
}

/// The first of `stem`, `stem-1`, `stem-2`, ... whose file, `-scaled` file and `-WxH` sub-size
/// files are all absent from `existing` (base names in the target upload directory).
#[must_use]
pub fn unique_stem(stem: &str, extension: &str, existing: &[&str]) -> String {
    let mut candidate = stem.to_owned();
    let mut number = 0_u32;
    while existing
        .iter()
        .any(|name| collides(name, &candidate, extension))
    {
        number += 1;
        candidate = format!("{stem}-{number}");
    }
    candidate
}

// Whether `name` is `stem.ext`, `stem-scaled.ext` or `stem-<digits>x<digits>.ext`.
fn collides(name: &str, stem: &str, extension: &str) -> bool {
    let Some(rest) = name.strip_prefix(stem) else {
        return false;
    };
    let Some(middle) = rest
        .strip_suffix(extension)
        .and_then(|rest| rest.strip_suffix('.'))
    else {
        return false;
    };
    if middle.is_empty() || middle == "-scaled" {
        return true;
    }
    middle
        .strip_prefix('-')
        .and_then(|dims| dims.split_once('x'))
        .is_some_and(|(w, h)| is_digits(w) && is_digits(h))
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// The reference's save-context slug for a title: accents folded, lower-cased, `.` to `-`,
/// only `[a-z0-9_-]` kept with other UTF-8 percent-encoded, whitespace and dash runs to one dash.
/// A feed name gets the `-2` suffix the reference gives a bad attachment slug.
#[must_use]
pub fn sanitize_slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in remove_accents(title).chars().flat_map(char::to_lowercase) {
        match c {
            'a'..='z' | '0'..='9' | '_' | '-' => out.push(c),
            '.' => out.push('-'),
            c if c.is_whitespace() => out.push('-'),
            c if !c.is_ascii() => {
                let mut buffer = [0_u8; 4];
                for byte in c.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("%{byte:02x}"));
                }
            }
            _ => {}
        }
    }
    let mut collapsed = String::with_capacity(out.len());
    for c in out.chars() {
        if !(c == '-' && collapsed.ends_with('-')) {
            collapsed.push(c);
        }
    }
    let slug = collapsed.trim_matches('-').to_owned();
    if FEEDS.contains(&slug.as_str()) {
        format!("{slug}-2")
    } else {
        slug
    }
}

fn is_known_extension(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    KNOWN_EXTENSIONS.contains(&lower.as_str())
}

// `foo.tar.php.jpg` keeps `tar` but becomes `foo.tar.php_.jpg`: a 2-5 letter (plus optional
// digit) intermediate part that is not a known extension gets `_` so it cannot act as one.
fn mark_intermediate_extensions(name: &str) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() <= 2 {
        return name.to_owned();
    }
    let mut out = parts[0].to_owned();
    for part in &parts[1..parts.len() - 1] {
        out.push('.');
        out.push_str(part);
        if looks_like_extension(part) && !is_known_extension(part) {
            out.push('_');
        }
    }
    out.push('.');
    out.push_str(parts[parts.len() - 1]);
    out
}

fn looks_like_extension(part: &str) -> bool {
    let bytes = part.as_bytes();
    let letters = bytes.iter().take_while(|b| b.is_ascii_alphabetic()).count();
    let rest = &bytes[letters..];
    (2..=5).contains(&letters) && (rest.is_empty() || (rest.len() == 1 && rest[0].is_ascii_digit()))
}

fn collapse_dots(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if !(c == '.' && out.ends_with('.')) {
            out.push(c);
        }
    }
    out
}

// `[\r\n\t -]+` becomes a single dash.
fn collapse_space_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if matches!(c, '\r' | '\n' | '\t' | ' ' | '-') {
            if !in_run {
                out.push('-');
                in_run = true;
            }
        } else {
            in_run = false;
            out.push(c);
        }
    }
    out
}

/// Folds Latin-1 Supplement and Latin Extended-A letters to ASCII as the reference does.
fn remove_accents(text: &str) -> String {
    if text.is_ascii() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match fold(c) {
            Some(ascii) => out.push_str(ascii),
            None => out.push(c),
        }
    }
    out
}

fn fold(c: char) -> Option<&'static str> {
    let code = u32::from(c);
    match code {
        0xC0..=0xFF => latin1(c),
        0x100..=0x17F => extended_a(code),
        _ => None,
    }
}

fn latin1(c: char) -> Option<&'static str> {
    Some(match c {
        'À'..='Å' => "A",
        'Æ' => "AE",
        'Ç' => "C",
        'È'..='Ë' => "E",
        'Ì'..='Ï' => "I",
        'Ð' => "D",
        'Ñ' => "N",
        'Ò'..='Ö' | 'Ø' => "O",
        'Ù'..='Ü' => "U",
        'Ý' => "Y",
        'Þ' => "TH",
        'ß' => "s",
        'à'..='å' => "a",
        'æ' => "ae",
        'ç' => "c",
        'è'..='ë' => "e",
        'ì'..='ï' => "i",
        'ð' => "d",
        'ñ' => "n",
        'ò'..='ö' | 'ø' => "o",
        'ù'..='ü' => "u",
        'ý' | 'ÿ' => "y",
        'þ' => "th",
        _ => return None,
    })
}

// Latin Extended-A alternates upper/lower case pairs; the letter depends on the code range.
fn extended_a(code: u32) -> Option<&'static str> {
    let upper = code % 2 == 0;
    let pair = |up: &'static str, low: &'static str| Some(if upper { up } else { low });
    match code {
        0x100..=0x105 => pair("A", "a"),
        0x106..=0x10D => pair("C", "c"),
        0x10E..=0x111 => pair("D", "d"),
        0x112..=0x11B => pair("E", "e"),
        0x11C..=0x123 => pair("G", "g"),
        0x124..=0x127 => pair("H", "h"),
        0x128..=0x131 => pair("I", "i"),
        0x132..=0x133 => pair("IJ", "ij"),
        0x134..=0x135 => pair("J", "j"),
        0x136..=0x137 => pair("K", "k"),
        0x138 => Some("k"),
        // From here pairs start on odd code points (upper odd, lower even).
        0x139..=0x142 => Some(if upper { "l" } else { "L" }),
        0x143..=0x148 => Some(if upper { "n" } else { "N" }),
        0x149 => Some("n"),
        0x14A..=0x14B => pair("N", "n"),
        0x14C..=0x151 => pair("O", "o"),
        0x152..=0x153 => pair("OE", "oe"),
        0x154..=0x159 => pair("R", "r"),
        0x15A..=0x161 => pair("S", "s"),
        0x162..=0x167 => pair("T", "t"),
        0x168..=0x173 => pair("U", "u"),
        0x174..=0x175 => pair("W", "w"),
        0x176..=0x177 => pair("Y", "y"),
        0x178 => Some("Y"),
        0x179..=0x17E => Some(if upper { "z" } else { "Z" }),
        0x17F => Some("s"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        remove_accents, sanitize_file_name, sanitize_slug, scaled_name, sized_name, split,
        unique_stem,
    };
    use crate::Error;

    fn clean(name: &str) -> String {
        sanitize_file_name(name).unwrap_or_default()
    }

    #[test]
    fn special_characters_and_spaces_follow_the_reference() {
        assert_eq!(clean("My Photo (1).JPG"), "My-Photo-1.JPG");
        assert_eq!(clean("a  b\t-c.png"), "a-b-c.png");
        assert_eq!(clean("a%20b.png"), "a20b.png");
        assert_eq!(clean("a+b.png"), "ab.png");
        assert_eq!(clean("..hidden..png"), "hidden.png");
        assert_eq!(clean("-_name_-.gif"), "name_-.gif");
        assert_eq!(clean("name.gif._-"), "name.gif");
        assert_eq!(clean("caf\u{e9} cr\u{e8}me.jpg"), "cafe-creme.jpg");
        assert_eq!(clean("a\u{a0}b.jpg"), "a-b.jpg");
        assert_eq!(clean("\u{201c}q\u{201d}.webp"), "q.webp");
        assert_eq!(clean("noext"), "noext");
        assert_eq!(clean("jpg"), "unnamed-file.jpg");
        assert_eq!(sanitize_file_name("?!*"), Err(Error::Name));
    }

    #[test]
    fn unknown_intermediate_extensions_get_an_underscore() {
        assert_eq!(clean("shell.php.jpg"), "shell.php_.jpg");
        assert_eq!(clean("doc.pdf.jpg"), "doc.pdf.jpg");
        assert_eq!(clean("a.mp4.png"), "a.mp4.png");
        assert_eq!(clean("a.abcd1.png"), "a.abcd1_.png");
        assert_eq!(clean("v.1.2.png"), "v.1.2.png");
        assert_eq!(clean("a.abcdef.png"), "a.abcdef.png");
        assert_eq!(clean("a.ab12.png"), "a.ab12.png");
    }

    #[test]
    fn accents_fold_in_both_latin_blocks() {
        assert_eq!(remove_accents("plain"), "plain");
        assert_eq!(remove_accents("\u{c6}\u{de}\u{df}\u{f8}\u{ff}"), "AETHsoy");
        assert_eq!(remove_accents("\u{d7}"), "\u{d7}");
        assert_eq!(
            remove_accents("\u{100}\u{101}\u{10c}\u{10d}\u{132}\u{133}\u{138}"),
            "AaCcIJijk"
        );
        assert_eq!(
            remove_accents("\u{141}\u{142}\u{147}\u{148}\u{149}\u{14a}\u{14b}"),
            "LlNnnNn"
        );
        assert_eq!(
            remove_accents("\u{152}\u{153}\u{160}\u{161}\u{178}\u{17d}\u{17e}\u{17f}"),
            "OEoeSsYZzs"
        );
        assert_eq!(
            remove_accents("\u{10e}\u{11c}\u{124}\u{128}\u{134}\u{136}"),
            "DGHIJK"
        );
        assert_eq!(
            remove_accents("\u{112}\u{14c}\u{154}\u{162}\u{168}\u{174}\u{176}"),
            "EORTUWY"
        );
        assert_eq!(remove_accents("\u{3b1}"), "\u{3b1}");
    }

    #[test]
    fn sub_size_and_scaled_names() {
        assert_eq!(sized_name("photo", "jpg", 150, 150), "photo-150x150.jpg");
        assert_eq!(scaled_name("photo", "png"), "photo-scaled.png");
        assert_eq!(split("a.b.png"), ("a.b", "png"));
        assert_eq!(split("noext"), ("noext", ""));
    }

    #[test]
    fn unique_stems_avoid_files_and_their_sub_sizes() {
        assert_eq!(unique_stem("photo", "jpg", &[]), "photo");
        assert_eq!(unique_stem("photo", "jpg", &["photo.jpg"]), "photo-1");
        assert_eq!(
            unique_stem("photo", "jpg", &["photo-300x200.jpg", "photo-1-scaled.jpg"]),
            "photo-2"
        );
        assert_eq!(
            unique_stem(
                "photo",
                "jpg",
                &["photo.png", "photo-x.jpg", "photo-3x.jpg"]
            ),
            "photo"
        );
        assert_eq!(unique_stem("photo", "jpg", &["photo-1x2.jpg.bak"]), "photo");
        assert_eq!(unique_stem("photo", "jpg", &["other.jpg"]), "photo");
    }

    #[test]
    fn slugs_follow_the_save_context() {
        assert_eq!(sanitize_slug("My-Photo-1"), "my-photo-1");
        assert_eq!(sanitize_slug("a.b  c__d"), "a-b-c__d");
        assert_eq!(sanitize_slug("Caf\u{e9}!"), "cafe");
        assert_eq!(sanitize_slug("\u{3b1}"), "%ce%b1");
        assert_eq!(sanitize_slug("--x--"), "x");
        assert_eq!(sanitize_slug("feed"), "feed-2");
    }

    proptest::proptest! {
        #[test]
        fn sanitized_names_never_hold_special_characters(name in "\\PC{0,40}") {
            if let Ok(clean) = sanitize_file_name(&name) {
                proptest::prop_assert!(!clean.is_empty());
                proptest::prop_assert!(!clean.contains('/'));
                proptest::prop_assert!(!clean.contains(' '));
                proptest::prop_assert!(!clean.contains(".."));
                proptest::prop_assert!(!clean.starts_with('.'));
            }
        }
    }
}
