//! JSON Schema → GBNF, for the subset of keywords the query schema uses.
//!
//! llama.cpp's own converter lives in its `common` library, which this crate leaves
//! out (no downloader, no OpenMP). This one covers exactly the keywords T4.1's
//! `schema.json` uses and refuses any other, so a schema change that needs more can't
//! be skipped silently.
//!
//! The grammar is built as a small tree ([`Expr`]), rendered to GBNF for llama.cpp,
//! and can also be run directly ([`Grammar::accepts`]) so tests can prove what it
//! accepts without a model.
//!
//! What it generates is compact JSON (no whitespace), with an object's keys in the
//! schema's order: required keys always, optional keys may be left out.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use serde_json::Value as Json;

/// Longest free string the model may write (a merchant, an item name).
pub const MAX_STRING: u32 = 48;

/// Why a schema can't be turned into a grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GbnfError {
    /// A keyword outside the supported subset.
    Unsupported(String),
    /// A `$ref` to a definition that isn't there.
    MissingDef(String),
    /// A schema value of the wrong shape.
    Malformed(String),
}

impl fmt::Display for GbnfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(k) => write!(f, "unsupported schema keyword or form: {k}"),
            Self::MissingDef(d) => write!(f, "missing schema definition {d}"),
            Self::Malformed(w) => write!(f, "malformed schema at {w}"),
        }
    }
}

impl std::error::Error for GbnfError {}

/// One grammar expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// Exact text.
    Lit(String),
    /// One character in (or, if `negated`, outside) the inclusive ranges.
    Class {
        ranges: Vec<(char, char)>,
        negated: bool,
    },
    Seq(Vec<Expr>),
    Alt(Vec<Expr>),
    /// A named rule.
    Ref(String),
    /// `min` to `max` repeats.
    Repeat {
        expr: Box<Expr>,
        min: u32,
        max: u32,
    },
}

fn lit(s: &str) -> Expr {
    Expr::Lit(s.to_owned())
}

fn seq(v: Vec<Expr>) -> Expr {
    Expr::Seq(v)
}

fn class(ranges: &[(char, char)]) -> Expr {
    Expr::Class {
        ranges: ranges.to_vec(),
        negated: false,
    }
}

/// A set of named rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grammar {
    rules: BTreeMap<String, Expr>,
}

impl Grammar {
    pub fn rule(&self, name: &str) -> Option<&Expr> {
        self.rules.get(name)
    }

    /// Adds (or replaces) a rule.
    pub fn set(&mut self, name: &str, expr: Expr) {
        self.rules.insert(name.to_owned(), expr);
    }

    /// The grammar in llama.cpp's GBNF, `root` first.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut names: Vec<&String> = self.rules.keys().collect();
        names.sort_by_key(|n| (n.as_str() != "root", n.as_str()));
        for name in names {
            out.push_str(name);
            out.push_str(" ::= ");
            render(&self.rules[name], &mut out, false);
            out.push('\n');
        }
        out
    }

    /// Whether `rule` matches all of `text`. A plain recogniser over the same tree
    /// that is rendered, for tests.
    pub fn accepts(&self, rule: &str, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut memo = HashMap::new();
        self.ends(&Expr::Ref(rule.to_owned()), 0, &chars, &mut memo)
            .contains(&chars.len())
    }

    fn ends(
        &self,
        e: &Expr,
        at: usize,
        s: &[char],
        memo: &mut HashMap<(String, usize), BTreeSet<usize>>,
    ) -> BTreeSet<usize> {
        match e {
            Expr::Lit(t) => {
                let t: Vec<char> = t.chars().collect();
                if s.len() >= at + t.len() && s[at..at + t.len()] == t[..] {
                    BTreeSet::from([at + t.len()])
                } else {
                    BTreeSet::new()
                }
            }
            Expr::Class { ranges, negated } => match s.get(at) {
                Some(c) if ranges.iter().any(|(a, b)| a <= c && c <= b) != *negated => {
                    BTreeSet::from([at + 1])
                }
                _ => BTreeSet::new(),
            },
            Expr::Seq(parts) => {
                let mut cur = BTreeSet::from([at]);
                for p in parts {
                    let mut next = BTreeSet::new();
                    for &i in &cur {
                        next.extend(self.ends(p, i, s, memo));
                    }
                    cur = next;
                    if cur.is_empty() {
                        break;
                    }
                }
                cur
            }
            Expr::Alt(options) => options
                .iter()
                .flat_map(|o| self.ends(o, at, s, memo))
                .collect(),
            Expr::Ref(name) => {
                let key = (name.clone(), at);
                if let Some(r) = memo.get(&key) {
                    return r.clone();
                }
                memo.insert(key.clone(), BTreeSet::new());
                let r = self.ends(&self.rules[name], at, s, memo);
                memo.insert(key, r.clone());
                r
            }
            Expr::Repeat { expr, min, max } => {
                let mut all = BTreeSet::new();
                let mut cur = BTreeSet::from([at]);
                for n in 0..=*max {
                    if n >= *min {
                        all.extend(cur.iter().copied());
                    }
                    if n == *max {
                        break;
                    }
                    let mut next = BTreeSet::new();
                    for &i in &cur {
                        next.extend(self.ends(expr, i, s, memo));
                    }
                    if next.is_empty() {
                        break;
                    }
                    cur = next;
                }
                all
            }
        }
    }
}

/// `s` as a GBNF string literal.
pub fn gbnf_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn class_char(c: char, out: &mut String) {
    match c {
        ']' | '[' | '\\' | '^' | '-' | '"' => {
            out.push('\\');
            out.push(c);
        }
        c if c.is_control() => out.push_str(&format!("\\x{:02X}", u32::from(c))),
        c => out.push(c),
    }
}

fn render(e: &Expr, out: &mut String, nested: bool) {
    match e {
        Expr::Lit(t) => out.push_str(&gbnf_literal(t)),
        Expr::Class { ranges, negated } => {
            out.push('[');
            if *negated {
                out.push('^');
            }
            for (a, b) in ranges {
                class_char(*a, out);
                if a != b {
                    out.push('-');
                    class_char(*b, out);
                }
            }
            out.push(']');
        }
        Expr::Seq(parts) if parts.is_empty() => out.push_str("\"\""),
        Expr::Seq(parts) => {
            if nested && parts.len() > 1 {
                out.push('(');
            }
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                render(p, out, true);
            }
            if nested && parts.len() > 1 {
                out.push(')');
            }
        }
        Expr::Alt(options) => {
            if nested {
                out.push('(');
            }
            for (i, o) in options.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                render(o, out, false);
            }
            if nested {
                out.push(')');
            }
        }
        Expr::Ref(name) => out.push_str(name),
        Expr::Repeat { expr, min, max } => {
            out.push('(');
            render(expr, out, false);
            out.push(')');
            out.push_str(&format!("{{{min},{max}}}"));
        }
    }
}

/// The JSON text of a constant, compact.
fn json_text(v: &Json) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// A JSON string with up to `max` characters.
fn json_string(max: u32) -> Expr {
    let plain = Expr::Class {
        ranges: vec![('"', '"'), ('\\', '\\'), ('\u{0}', '\u{1F}')],
        negated: true,
    };
    let hex = class(&[('0', '9'), ('a', 'f'), ('A', 'F')]);
    let escape = seq(vec![
        lit("\\"),
        Expr::Alt(vec![
            class(&[
                ('"', '"'),
                ('\\', '\\'),
                ('/', '/'),
                ('b', 'b'),
                ('f', 'f'),
                ('n', 'n'),
                ('r', 'r'),
                ('t', 't'),
            ]),
            seq(vec![lit("u"), hex.clone(), hex.clone(), hex.clone(), hex]),
        ]),
    ]);
    seq(vec![
        lit("\""),
        Expr::Repeat {
            expr: Box::new(Expr::Alt(vec![plain, escape])),
            min: 0,
            max,
        },
        lit("\""),
    ])
}

/// `^...$` patterns of `[0-9]`, `[A-Z]`, literals and `{n}` repeats, as a JSON string.
fn pattern(p: &str) -> Result<Expr, GbnfError> {
    let bad = || GbnfError::Unsupported(format!("pattern {p}"));
    let body = p
        .strip_prefix('^')
        .and_then(|b| b.strip_suffix('$'))
        .ok_or_else(bad)?;
    let mut parts = vec![lit("\"")];
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let one = match c {
            '[' => {
                let cls: String = chars.by_ref().take_while(|c| *c != ']').collect();
                match cls.as_str() {
                    "0-9" => class(&[('0', '9')]),
                    "A-Z" => class(&[('A', 'Z')]),
                    "a-z" => class(&[('a', 'z')]),
                    _ => return Err(bad()),
                }
            }
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' => {
                lit(&c.to_string())
            }
            _ => return Err(bad()),
        };
        let mut n = 1;
        if chars.peek() == Some(&'{') {
            chars.next();
            let count: String = chars.by_ref().take_while(|c| *c != '}').collect();
            n = count.parse().map_err(|_| bad())?;
        }
        for _ in 0..n {
            parts.push(one.clone());
        }
    }
    parts.push(lit("\""));
    Ok(seq(parts))
}

fn integer(min: Option<i64>, max: Option<i64>) -> Result<Expr, GbnfError> {
    let digits = |first: Expr| {
        seq(vec![
            first,
            Expr::Repeat {
                expr: Box::new(class(&[('0', '9')])),
                min: 0,
                max: 15,
            },
        ])
    };
    match (min, max) {
        (Some(a), Some(b)) if a <= b && b - a <= 31 => {
            Ok(Expr::Alt((a..=b).map(|n| lit(&n.to_string())).collect()))
        }
        (Some(1), None) => Ok(digits(class(&[('1', '9')]))),
        (Some(0), None) | (None, None) => {
            let positive = Expr::Alt(vec![lit("0"), digits(class(&[('1', '9')]))]);
            Ok(if min.is_none() {
                seq(vec![
                    Expr::Repeat {
                        expr: Box::new(lit("-")),
                        min: 0,
                        max: 1,
                    },
                    positive,
                ])
            } else {
                positive
            })
        }
        _ => Err(GbnfError::Unsupported(format!(
            "integer bounds {min:?}..{max:?}"
        ))),
    }
}

const KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "title",
    "description",
    "$ref",
    "$defs",
    "oneOf",
    "const",
    "enum",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
    "pattern",
];

/// Converts a schema to a grammar: one rule per `$defs` entry (named after it), plus
/// helper rules, and `root` for the schema itself.
pub fn from_schema(schema: &Json) -> Result<Grammar, GbnfError> {
    let mut c = Converter {
        defs: schema
            .get("$defs")
            .and_then(Json::as_object)
            .cloned()
            .unwrap_or_default(),
        grammar: Grammar::default(),
        done: BTreeSet::new(),
        next: 0,
    };
    let root = c.node(schema, "root")?;
    c.grammar.set("root", root);
    Ok(c.grammar)
}

/// Keys the model should write first, so it settles what an object is before its
/// details: an item's name, an ask's op, a scope's period.
const FIRST_KEYS: &[&str] = &["name", "op", "period"];

/// Where `key` goes in an object: [`FIRST_KEYS`] in that order, then the rest in
/// alphabetical order (the order serde_json's map keeps).
pub fn key_rank(key: &str) -> (usize, String) {
    let first = FIRST_KEYS
        .iter()
        .position(|k| *k == key)
        .unwrap_or(FIRST_KEYS.len());
    (first, key.to_owned())
}

/// `v` as compact JSON with object keys in the grammar's order and `null` members
/// left out: the text the grammar generates for the same value.
pub fn canonical(v: &Json) -> String {
    match v {
        Json::Object(map) => {
            let mut keys: Vec<(&String, &Json)> =
                map.iter().filter(|(_, v)| !v.is_null()).collect();
            keys.sort_by_key(|(k, _)| key_rank(k));
            let body: Vec<String> = keys
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        json_text(&Json::String((*k).clone())),
                        canonical(v)
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Json::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", body.join(","))
        }
        other => json_text(other),
    }
}

/// GBNF rule names are letters, digits and dashes.
pub fn rule_name(def: &str) -> String {
    def.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

struct Converter {
    defs: serde_json::Map<String, Json>,
    grammar: Grammar,
    done: BTreeSet<String>,
    next: usize,
}

impl Converter {
    fn fresh(&mut self, base: &str) -> String {
        self.next += 1;
        format!("{}-{}", rule_name(base), self.next)
    }

    fn def(&mut self, name: &str) -> Result<Expr, GbnfError> {
        let rule = rule_name(name);
        if self.done.insert(rule.clone()) {
            let schema = self
                .defs
                .get(name)
                .cloned()
                .ok_or_else(|| GbnfError::MissingDef(name.to_owned()))?;
            let e = self.node(&schema, name)?;
            self.grammar.set(&rule, e);
        }
        Ok(Expr::Ref(rule))
    }

    fn node(&mut self, s: &Json, at: &str) -> Result<Expr, GbnfError> {
        let obj = s
            .as_object()
            .ok_or_else(|| GbnfError::Malformed(at.to_owned()))?;
        for k in obj.keys() {
            if !KEYWORDS.contains(&k.as_str()) {
                return Err(GbnfError::Unsupported(k.clone()));
            }
        }
        if let Some(r) = obj.get("$ref") {
            let name = r
                .as_str()
                .and_then(|r| r.strip_prefix("#/$defs/"))
                .ok_or_else(|| GbnfError::Unsupported(format!("$ref {r}")))?;
            return self.def(name);
        }
        if let Some(options) = obj.get("oneOf") {
            let options = options
                .as_array()
                .ok_or_else(|| GbnfError::Malformed(at.to_owned()))?;
            return Ok(Expr::Alt(
                options
                    .iter()
                    .map(|o| self.node(o, at))
                    .collect::<Result<_, _>>()?,
            ));
        }
        if let Some(c) = obj.get("const") {
            return Ok(lit(&json_text(c)));
        }
        if let Some(e) = obj.get("enum") {
            let values = e
                .as_array()
                .ok_or_else(|| GbnfError::Malformed(at.to_owned()))?;
            return Ok(Expr::Alt(
                values.iter().map(|v| lit(&json_text(v))).collect(),
            ));
        }
        let types: Vec<&str> = match obj.get("type") {
            Some(Json::String(t)) => vec![t.as_str()],
            Some(Json::Array(ts)) => ts.iter().filter_map(Json::as_str).collect(),
            Some(_) => return Err(GbnfError::Malformed(at.to_owned())),
            None => return Err(GbnfError::Unsupported(format!("untyped node at {at}"))),
        };
        let mut options = Vec::new();
        for t in types {
            options.push(match t {
                "null" => lit("null"),
                "boolean" => Expr::Alt(vec![lit("true"), lit("false")]),
                "integer" => integer(
                    obj.get("minimum").and_then(Json::as_i64),
                    obj.get("maximum").and_then(Json::as_i64),
                )?,
                "string" => match obj.get("pattern").and_then(Json::as_str) {
                    Some(p) => pattern(p)?,
                    None => json_string(MAX_STRING),
                },
                "array" => self.array(obj, at)?,
                "object" => self.object(obj, at)?,
                other => return Err(GbnfError::Unsupported(format!("type {other}"))),
            });
        }
        Ok(if options.len() == 1 {
            options.remove(0)
        } else {
            Expr::Alt(options)
        })
    }

    fn array(&mut self, obj: &serde_json::Map<String, Json>, at: &str) -> Result<Expr, GbnfError> {
        let item = match obj.get("items") {
            Some(i) => self.node(i, at)?,
            None => {
                return Err(GbnfError::Unsupported(format!(
                    "array without items at {at}"
                )));
            }
        };
        let name = self.fresh(&format!("{at}-item"));
        self.grammar.set(&name, item);
        let item = Expr::Ref(name);
        let max = obj.get("maxItems").and_then(Json::as_u64).unwrap_or(64);
        let more = Expr::Repeat {
            expr: Box::new(seq(vec![lit(","), item.clone()])),
            min: 0,
            max: u32::try_from(max.saturating_sub(1)).unwrap_or(63),
        };
        let some = seq(vec![item, more]);
        let min = obj.get("minItems").and_then(Json::as_u64).unwrap_or(0);
        Ok(match min {
            0 => seq(vec![
                lit("["),
                Expr::Repeat {
                    expr: Box::new(some),
                    min: 0,
                    max: 1,
                },
                lit("]"),
            ]),
            1 => seq(vec![lit("["), some, lit("]")]),
            _ => return Err(GbnfError::Unsupported(format!("minItems {min}"))),
        })
    }

    /// Keys in schema order; required ones always, optional ones may be left out.
    /// Commas go between the keys present: rule `<name>-<i>-<s>` writes keys from the
    /// `i`th on, `s` saying whether one has been written already.
    fn object(&mut self, obj: &serde_json::Map<String, Json>, at: &str) -> Result<Expr, GbnfError> {
        if obj.get("additionalProperties") != Some(&Json::Bool(false)) {
            return Err(GbnfError::Unsupported(format!(
                "object without additionalProperties: false at {at}"
            )));
        }
        let props = obj
            .get("properties")
            .and_then(Json::as_object)
            .ok_or_else(|| GbnfError::Malformed(at.to_owned()))?;
        let required: BTreeSet<&str> = obj
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .collect();
        let mut members = Vec::new();
        let mut keys: Vec<(&String, &Json)> = props.iter().collect();
        keys.sort_by_key(|(k, _)| key_rank(k));
        for (k, v) in keys {
            let value = self.node(v, &format!("{at}-{k}"))?;
            members.push((
                seq(vec![
                    lit(&format!("{}:", json_text(&Json::String(k.clone())))),
                    value,
                ]),
                required.contains(k.as_str()),
            ));
        }
        let base = self.fresh(at);
        let name =
            |i: usize, written: bool| format!("{base}-{i}-{}", if written { "w" } else { "e" });
        let n = members.len();
        for i in (0..=n).rev() {
            for written in [false, true] {
                let e = if i == n {
                    seq(vec![])
                } else {
                    let (m, req) = &members[i];
                    let sep = if written { vec![lit(",")] } else { vec![] };
                    let mut with = sep;
                    with.push(m.clone());
                    with.push(Expr::Ref(name(i + 1, true)));
                    let with = seq(with);
                    if *req {
                        with
                    } else {
                        Expr::Alt(vec![with, Expr::Ref(name(i + 1, written))])
                    }
                };
                self.grammar.set(&name(i, written), e);
            }
        }
        Ok(seq(vec![lit("{"), Expr::Ref(name(0, false)), lit("}")]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grammar(schema: Json) -> Grammar {
        from_schema(&schema).unwrap()
    }

    #[test]
    fn objects_keep_order_and_allow_leaving_out_optional_keys() {
        let g = grammar(json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "minimum": 1},
                "b": {"enum": ["x", "y"]},
                "c": {"type": ["string", "null"]}
            },
            "required": ["b"],
            "additionalProperties": false
        }));
        for ok in [
            r#"{"b":"x"}"#,
            r#"{"a":12,"b":"y"}"#,
            r#"{"b":"x","c":null}"#,
            r#"{"a":1,"b":"x","c":"hi \"there\""}"#,
        ] {
            assert!(g.accepts("root", ok), "{ok}");
        }
        for bad in [
            r#"{}"#,
            r#"{"a":0,"b":"x"}"#,
            r#"{"b":"z"}"#,
            r#"{"b":"x","a":1}"#,
            r#"{"b":"x",}"#,
            r#"{"b":"x","d":1}"#,
            r#"{ "b":"x"}"#,
            r#"{"a":01,"b":"x"}"#,
        ] {
            assert!(!g.accepts("root", bad), "{bad}");
        }
    }

    #[test]
    fn refs_one_of_arrays_and_patterns() {
        let g = grammar(json!({
            "$ref": "#/$defs/P",
            "$defs": {
                "P": {"oneOf": [
                    {"enum": ["all_time"]},
                    {"type": "object", "properties": {"month": {"$ref": "#/$defs/YM"}},
                     "required": ["month"], "additionalProperties": false},
                    {"type": "object", "properties": {"q": {"type": "integer", "minimum": 1, "maximum": 4}},
                     "required": ["q"], "additionalProperties": false},
                    {"type": "array", "items": {"const": 7}, "minItems": 1, "maxItems": 3}
                ]},
                "YM": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}$"}
            }
        }));
        for ok in [
            r#""all_time""#,
            r#"{"month":"2025-07"}"#,
            r#"{"q":4}"#,
            "[7]",
            "[7,7,7]",
        ] {
            assert!(g.accepts("root", ok), "{ok}");
        }
        for bad in [
            r#"{"month":"2025-7"}"#,
            r#"{"q":5}"#,
            "[]",
            "[7,7,7,7]",
            r#""last""#,
        ] {
            assert!(!g.accepts("root", bad), "{bad}");
        }
        assert!(g.rule("YM").is_some() && g.rule("P").is_some());
    }

    #[test]
    fn unknown_keywords_are_refused() {
        for schema in [
            json!({"type": "string", "format": "date"}),
            json!({"type": "object", "properties": {}}),
            json!({"anyOf": []}),
            json!({"$ref": "#/$defs/Missing"}),
            json!({"type": "integer", "minimum": 5}),
        ] {
            assert!(from_schema(&schema).is_err(), "{schema}");
        }
    }

    #[test]
    fn strings_are_capped_and_escaped() {
        let g = grammar(json!({"type": "string"}));
        assert!(g.accepts("root", r#""a\\bé ✓""#));
        assert!(!g.accepts("root", "\"a\nb\""));
        assert!(g.accepts("root", &format!("\"{}\"", "x".repeat(MAX_STRING as usize))));
        assert!(!g.accepts(
            "root",
            &format!("\"{}\"", "x".repeat(MAX_STRING as usize + 1))
        ));
    }

    #[test]
    fn renders_gbnf() {
        let g = grammar(json!({"enum": ["a\"b", 3]}));
        assert_eq!(g.render(), "root ::= \"\\\"a\\\\\\\"b\\\"\" | \"3\"\n");
        let g = grammar(json!({"type": "string", "pattern": "^[A-Z]{2}$"}));
        assert_eq!(g.render(), "root ::= \"\\\"\" [A-Z] [A-Z] \"\\\"\"\n");
    }
}
