//! RFC 4180 tokenising, plus what bank files need: a delimiter other than `,`, CRLF
//! or LF line ends, and line breaks inside quoted fields.

/// One record: its fields and the 1-based line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    pub line: usize,
    pub fields: Vec<String>,
}

/// Splits `text` into records. A quote inside an unquoted field is kept as text, as
/// spreadsheet programs do; a quoted field ends at its closing quote, and anything
/// before the next delimiter is appended. Blank lines are skipped.
pub fn records(text: &str, delimiter: char, first_line: usize) -> Vec<RawRecord> {
    let mut out = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut field_started_quoted = false;
    let mut line = first_line;
    let mut record_line = first_line;
    let mut chars = text.chars().peekable();
    let end_record =
        |fields: &mut Vec<String>, field: &mut String, start: usize, out: &mut Vec<RawRecord>| {
            fields.push(std::mem::take(field));
            let fs = std::mem::take(fields);
            if !(fs.len() == 1 && fs[0].is_empty()) {
                out.push(RawRecord {
                    line: start,
                    fields: fs,
                });
            }
        };
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                '\r' if chars.peek() == Some(&'\n') => {}
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !field_started_quoted => {
                in_quotes = true;
                field_started_quoted = true;
            }
            c if c == delimiter => {
                fields.push(std::mem::take(&mut field));
                field_started_quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                end_record(&mut fields, &mut field, record_line, &mut out);
                field_started_quoted = false;
                line += 1;
                record_line = line;
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !fields.is_empty() || field_started_quoted {
        end_record(&mut fields, &mut field, record_line, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(text: &str) -> Vec<Vec<String>> {
        records(text, ',', 1)
            .into_iter()
            .map(|r| r.fields)
            .collect()
    }

    #[test]
    fn quoting_and_line_ends() {
        assert_eq!(
            fields("a,\"b,c\",d\r\n1,2,3\n"),
            vec![vec!["a", "b,c", "d"], vec!["1", "2", "3"]]
        );
        assert_eq!(
            fields("\"say \"\"hi\"\"\",x\n"),
            vec![vec!["say \"hi\"", "x"]]
        );
        assert_eq!(fields("a,,\n"), vec![vec!["a", "", ""]]);
        assert_eq!(fields("\"\",x"), vec![vec!["", "x"]]);
    }

    #[test]
    fn newlines_inside_quotes_keep_line_numbers() {
        let r = records("h1,h2\n\"two\nlines\",x\nnext,y\n", ',', 1);
        assert_eq!(r[1].fields, vec!["two\nlines", "x"]);
        assert_eq!((r[0].line, r[1].line, r[2].line), (1, 2, 4));
    }

    #[test]
    fn blank_lines_are_skipped_and_stray_quotes_kept() {
        assert_eq!(fields("a\n\nb\n"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(fields("ab\"c,d\n"), vec![vec!["ab\"c", "d"]]);
    }
}
