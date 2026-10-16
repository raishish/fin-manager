//! JSON output with one record per line.
//!
//! The data files hold thousands of records. Pretty-printing every field would make
//! them several times larger, and fully compact JSON makes diffs unreadable. This
//! formatter expands only the outer levels: the top object, its arrays, and one more
//! level of arrays (for `taxonomy.categories`), so each transaction or truth record sits
//! compact on its own line. The output is ordinary JSON.

use std::io;

use serde::Serialize;
use serde_json::ser::Formatter;

/// Arrays at depth 0..=2 and objects at depth 0..=1 are expanded; deeper ones are
/// written compactly.
struct LineFormatter {
    /// For each open container: whether it is expanded, and whether it has any values.
    stack: Vec<(bool, bool)>,
}

impl LineFormatter {
    fn expanded(&self) -> bool {
        self.stack.last().is_some_and(|&(expanded, _)| expanded)
    }

    fn open(&mut self, max_depth: usize) {
        let depth = self.stack.len();
        let expanded = (depth == 0 || self.expanded()) && depth <= max_depth;
        self.stack.push((expanded, false));
    }

    fn close<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        if let Some((true, true)) = self.stack.pop() {
            self.newline(w, self.stack.len())?;
        }
        Ok(())
    }

    /// Separator and line break before the next value or key.
    fn next_item<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        if let Some(top) = self.stack.last_mut() {
            top.1 = true;
        }
        if self.expanded() {
            self.newline(w, self.stack.len())?;
        }
        Ok(())
    }

    fn newline<W: ?Sized + io::Write>(&self, w: &mut W, depth: usize) -> io::Result<()> {
        w.write_all(b"\n")?;
        for _ in 0..depth {
            w.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl Formatter for LineFormatter {
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.open(2);
        w.write_all(b"[")
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.close(w)?;
        w.write_all(b"]")
    }

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.next_item(w, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.open(1);
        w.write_all(b"{")
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.close(w)?;
        w.write_all(b"}")
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.next_item(w, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(if self.expanded() { b": " } else { b":" })
    }
}

/// Serialises `value` with one record per line, ending in a newline.
pub fn to_lines<T: Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ser =
        serde_json::Serializer::with_formatter(&mut out, LineFormatter { stack: Vec::new() });
    value
        .serialize(&mut ser)
        .expect("serialising plain data to memory cannot fail");
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expands_outer_levels_only() {
        let v = json!({
            "a": [{"x": 1, "y": [1, 2]}, {"x": 2, "y": []}],
            "t": {"v": {"b": "1"}, "c": [{"k": "z"}]},
            "e": []
        });
        let s = String::from_utf8(to_lines(&v)).unwrap();
        let expected = "{\n  \"a\": [\n    {\"x\":1,\"y\":[1,2]},\n    {\"x\":2,\"y\":[]}\n  ],\n  \"e\": [],\n  \"t\": {\n    \"c\": [\n      {\"k\":\"z\"}\n    ],\n    \"v\": {\"b\":\"1\"}\n  }\n}\n";
        assert_eq!(s, expected);
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }
}
