//! A tolerant reader for the OFX body, from `<OFX>` on: OFX 1.x SGML (leaf elements
//! with or without closing tags) and OFX 2.x XML, one element per line or all on one
//! line.
//!
//! An element followed directly by text is a leaf holding that text (trimmed, entities
//! decoded). Its closing tag is optional. Any other element is an aggregate that lasts
//! until its closing tag; a closing tag also closes every unclosed aggregate inside it.
//! Stray closing tags are ignored. Tag names are compared in upper case. Comments,
//! CDATA sections and processing instructions are handled as XML defines them.

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Element {
    pub name: String,
    /// The text of a leaf; `None` for an aggregate.
    pub value: Option<String>,
    pub children: Vec<Element>,
}

impl Element {
    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every child named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The text of the leaf at `path` (child names, outermost first), if non-empty.
    pub fn text(&self, path: &[&str]) -> Option<&str> {
        let mut node = self;
        for name in path {
            node = node.child(name)?;
        }
        node.value.as_deref().filter(|v| !v.is_empty())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Open(String),
    Close(String),
    Text(String),
}

/// Decodes `&amp; &lt; &gt; &quot; &apos; &nbsp;` and numeric references. An `&` that
/// starts none of these is kept as written (SGML files often contain a bare `AT&T`).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{A0}'),
                _ => {
                    let code = if let Some(hex) =
                        entity.strip_prefix("#x").or(entity.strip_prefix("#X"))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        entity.strip_prefix('#').and_then(|d| d.parse().ok())
                    };
                    code.and_then(char::from_u32)
                }
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(body: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut rest = body;
    loop {
        let Some(lt) = rest.find('<') else {
            let text = rest.trim();
            if !text.is_empty() {
                tokens.push(Token::Text(unescape(text)));
            }
            return Ok(tokens);
        };
        let text = rest[..lt].trim();
        if !text.is_empty() {
            tokens.push(Token::Text(unescape(text)));
        }
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after
                .find("-->")
                .ok_or(Error::Malformed("unterminated comment"))?;
            rest = &after[end + 3..];
        } else if let Some(after) = rest.strip_prefix("<![CDATA[") {
            let end = after
                .find("]]>")
                .ok_or(Error::Malformed("unterminated CDATA section"))?;
            let text = after[..end].trim();
            if !text.is_empty() {
                tokens.push(Token::Text(text.to_owned()));
            }
            rest = &after[end + 3..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest
                .find('>')
                .ok_or(Error::Malformed("unterminated declaration"))?;
            rest = &rest[end + 1..];
        } else {
            let end = rest.find('>').ok_or(Error::Malformed("unterminated tag"))?;
            let tag = rest[1..end].trim();
            rest = &rest[end + 1..];
            if let Some(name) = tag.strip_prefix('/') {
                tokens.push(Token::Close(name.trim().to_ascii_uppercase()));
            } else {
                // `<TAG/>` (XML empty element) is an empty leaf.
                let (name, empty) = match tag.strip_suffix('/') {
                    Some(n) => (n.trim(), true),
                    None => (tag, false),
                };
                // Attributes aren't used in OFX; keep only the name.
                let name = name.split_whitespace().next().unwrap_or("");
                if name.is_empty() {
                    return Err(Error::Malformed("empty tag"));
                }
                let name = name.to_ascii_uppercase();
                tokens.push(Token::Open(name.clone()));
                if empty {
                    tokens.push(Token::Close(name));
                }
            }
        }
    }
}

/// Builds the element tree of an OFX body (text starting at `<OFX>`).
pub(crate) fn parse(body: &str) -> Result<Element, Error> {
    let tokens = tokenize(body)?;
    let mut stack = vec![Element::default()]; // a synthetic root holding <OFX>
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Open(name) => {
                if let Some(Token::Text(value)) = tokens.get(i + 1) {
                    // A leaf. Its closing tag, if any, is consumed with it.
                    stack.last_mut().expect("root").children.push(Element {
                        name: name.clone(),
                        value: Some(value.clone()),
                        children: Vec::new(),
                    });
                    i += 2;
                    if matches!(tokens.get(i), Some(Token::Close(c)) if c == name) {
                        i += 1;
                    }
                    continue;
                }
                stack.push(Element {
                    name: name.clone(),
                    value: None,
                    children: Vec::new(),
                });
            }
            Token::Close(name) => {
                // Close back to the matching aggregate; ignore stray closing tags.
                if let Some(depth) = stack.iter().rposition(|e| &e.name == name)
                    && depth > 0
                {
                    while stack.len() > depth {
                        let mut done = stack.pop().expect("deeper than depth");
                        // `<MEMO></MEMO>`: an aggregate with nothing inside is an empty
                        // leaf.
                        if done.children.is_empty() && done.value.is_none() {
                            done.value = Some(String::new());
                        }
                        stack.last_mut().expect("root").children.push(done);
                    }
                }
            }
            Token::Text(_) => {
                // Text not directly after an opening tag (after a closing tag, say).
                // OFX has no mixed content, so it carries nothing.
            }
        }
        i += 1;
    }
    // Close whatever the file left open (truncated SGML is common).
    while stack.len() > 1 {
        let done = stack.pop().expect("len > 1");
        stack.last_mut().expect("root").children.push(done);
    }
    let mut root = stack.pop().expect("root");
    root.children
        .drain(..)
        .find(|e| e.name == "OFX")
        .ok_or(Error::Malformed("no OFX element"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(name: &str, value: &str) -> Element {
        Element {
            name: name.into(),
            value: Some(value.into()),
            children: vec![],
        }
    }

    fn agg(name: &str, children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            value: None,
            children,
        }
    }

    #[test]
    fn sgml_leaves_with_and_without_closing_tags_read_the_same() {
        let expected = agg(
            "OFX",
            vec![agg(
                "STMTTRN",
                vec![leaf("TRNAMT", "-1.00"), leaf("NAME", "A & B")],
            )],
        );
        for body in [
            "<OFX>\n<STMTTRN>\n<TRNAMT>-1.00\n<NAME>A &amp; B\n</STMTTRN>\n</OFX>",
            "<OFX><STMTTRN><TRNAMT>-1.00<NAME>A &amp; B</STMTTRN></OFX>",
            "<OFX><STMTTRN><TRNAMT>-1.00</TRNAMT><NAME>A &amp; B</NAME></STMTTRN></OFX>",
            "<ofx>\r\n  <stmttrn>\r\n    <trnamt> -1.00 </trnamt>\r\n    <name>A & B   \r\n  </stmttrn>\r\n</ofx>",
            "<OFX><!-- note --><STMTTRN><TRNAMT><![CDATA[-1.00]]></TRNAMT><NAME>A &#38; B<?pi x?></STMTTRN></OFX>",
        ] {
            assert_eq!(parse(body).unwrap(), expected, "{body}");
        }
    }

    #[test]
    fn closing_an_aggregate_closes_what_is_open_inside_it() {
        // <INNER> is never closed; </OUTER> closes it.
        let t = parse("<OFX><OUTER><INNER><A>1</OUTER><B>2</OFX>").unwrap();
        assert_eq!(t.text(&["OUTER", "INNER", "A"]), Some("1"));
        assert_eq!(t.text(&["B"]), Some("2"));
        // Truncated files are closed at the end; stray closing tags are ignored.
        let t = parse("<OFX></NOPE><X><Y>3").unwrap();
        assert_eq!(t.text(&["X", "Y"]), Some("3"));
    }

    #[test]
    fn empty_elements_are_empty_leaves() {
        let t = parse("<OFX><NAME></NAME><MEMO/><SIC>5411</OFX>").unwrap();
        assert_eq!(t.child("NAME").unwrap().value.as_deref(), Some(""));
        assert_eq!(t.child("MEMO").unwrap().value.as_deref(), Some(""));
        assert_eq!(t.text(&["NAME"]), None);
        assert_eq!(t.text(&["SIC"]), Some("5411"));
    }

    #[test]
    fn entities() {
        assert_eq!(
            unescape("AT&amp;T &lt;x&gt; &quot;q&quot; &apos;s&apos;"),
            "AT&T <x> \"q\" 's'"
        );
        assert_eq!(unescape("&#65;&#x42;&#X43;"), "ABC");
        // Not entities: kept as written.
        assert_eq!(unescape("AT&T"), "AT&T");
        assert_eq!(unescape("A & B"), "A & B");
        assert_eq!(unescape("&unknown;"), "&unknown;");
        assert_eq!(unescape("&#xFFFFFFFF;"), "&#xFFFFFFFF;");
        assert_eq!(unescape("trailing &"), "trailing &");
    }

    #[test]
    fn malformed_bodies_are_errors_not_panics() {
        for body in [
            "<OFX><A",
            "<OFX><!-- x",
            "<OFX><![CDATA[x",
            "<OFX><>",
            "<NOTOFX>1</NOTOFX>",
            "",
        ] {
            assert!(parse(body).is_err(), "{body:?}");
        }
    }
}
