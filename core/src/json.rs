//! Minimal JSON value, parser and pretty printer.
//! Just enough for our config file; avoids pulling serde into a tiny binary.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Keeps insertion order so saved files stay readable and diff-friendly.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }
    /// String field or "" (handy for optional text fields).
    pub fn str_or_empty(&self, key: &str) -> String {
        self.get(key).and_then(Json::as_str).unwrap_or("").to_string()
    }

    pub fn parse(src: &str) -> Result<Json, String> {
        let mut p = Parser { s: src.as_bytes(), i: 0, depth: 0 };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(v)
    }

    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    /// Single line, no spaces (for messages).
    pub fn compact(&self) -> String {
        let mut out = String::new();
        self.write_compact(&mut out);
        out
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            Json::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write_compact(out);
                }
                out.push(']');
            }
            Json::Obj(v) => {
                out.push('{');
                for (i, (k, x)) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(out, k);
                    out.push(':');
                    x.write_compact(out);
                }
                out.push('}');
            }
            scalar => scalar.write(out, 0),
        }
    }

    fn write(&self, out: &mut String, indent: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => write_str(out, s),
            Json::Arr(v) if v.is_empty() => out.push_str("[]"),
            Json::Arr(v) if v.iter().all(Json::is_scalar) => {
                // short scalar arrays on one line: [50, 50]
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.write(out, indent);
                }
                out.push(']');
            }
            Json::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    out.push_str(if i > 0 { ",\n" } else { "\n" });
                    pad(out, indent + 1);
                    x.write(out, indent + 1);
                }
                out.push('\n');
                pad(out, indent);
                out.push(']');
            }
            Json::Obj(v) if v.is_empty() => out.push_str("{}"),
            Json::Obj(v) => {
                out.push('{');
                for (i, (k, x)) in v.iter().enumerate() {
                    out.push_str(if i > 0 { ",\n" } else { "\n" });
                    pad(out, indent + 1);
                    write_str(out, k);
                    out.push_str(": ");
                    x.write(out, indent + 1);
                }
                out.push('\n');
                pad(out, indent);
                out.push('}');
            }
        }
    }

    fn is_scalar(&self) -> bool {
        !matches!(self, Json::Arr(_) | Json::Obj(_))
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Deeper nesting is rejected: a crafted file must not blow the stack.
const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        format!("JSON error at byte {}: {msg}", self.i)
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end")),
            Some(b'{' | b'[') if self.depth >= MAX_DEPTH => Err(self.err("nested too deeply")),
            Some(b'{') => {
                self.depth += 1;
                let r = self.object();
                self.depth -= 1;
                r
            }
            Some(b'[') => {
                self.depth += 1;
                let r = self.array();
                self.depth -= 1;
                r
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }
    fn object(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.eat("}") {
            return Ok(Json::Obj(v));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(self.err("expected key"));
            }
            let k = self.string()?;
            self.ws();
            if !self.eat(":") {
                return Err(self.err("expected ':'"));
            }
            self.ws();
            v.push((k, self.value()?));
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("}") {
                return Ok(Json::Obj(v));
            }
            return Err(self.err("expected ',' or '}'"));
        }
    }
    fn array(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut v = Vec::new();
        self.ws();
        if self.eat("]") {
            return Ok(Json::Arr(v));
        }
        loop {
            self.ws();
            v.push(self.value()?);
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("]") {
                return Ok(Json::Arr(v));
            }
            return Err(self.err("expected ',' or ']'"));
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let txt = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
        txt.parse::<f64>().map(Json::Num).map_err(|_| self.err("bad number"))
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        let v = u32::from_str_radix(std::str::from_utf8(h).unwrap_or(""), 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            // copy the run of plain bytes (keeps UTF-8 intact)
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|_| self.err("invalid UTF-8"))?);
            match self.s.get(self.i) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => {}
            }
            self.i += 1; // backslash
            let c = *self.s.get(self.i).ok_or_else(|| self.err("bad escape"))?;
            self.i += 1;
            match c {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let mut cp = self.hex4()?;
                    if (0xD800..0xDC00).contains(&cp) && self.eat("\\u") {
                        let lo = self.hex4()?;
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                    }
                    out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                }
                _ => return Err(self.err("bad escape")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let src = r#"{"a": [1, 2.5, -3], "b": {"c": "x\"y\\z\n", "d": true, "e": null}, "f": [], "g": "çğış €"}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").unwrap().as_arr().unwrap()[1].as_f64(), Some(2.5));
        assert_eq!(v.get("b").unwrap().get("c").unwrap().as_str(), Some("x\"y\\z\n"));
        assert_eq!(v.get("g").unwrap().as_str(), Some("çğış €"));
        assert_eq!(Json::parse(&v.pretty()).unwrap(), v);
        assert_eq!(Json::parse(&v.compact()).unwrap(), v);
        assert!(!v.compact().contains('\n'));
    }

    #[test]
    fn unicode_escapes() {
        // build the escapes at runtime so the source holds a real backslash-u sequence
        let bs = '\x5C'; // backslash
        let src = format!("\"{bs}u00e7 {bs}ud83d{bs}ude00\"");
        assert!(src.contains("u00e7") && !src.contains('\u{e7}'));
        let v = Json::parse(&src).unwrap();
        assert_eq!(v.as_str(), Some("\u{e7} \u{1F600}"));
    }

    #[test]
    fn deep_nesting_is_rejected_not_a_crash() {
        let deep = "[".repeat(100_000) + &"]".repeat(100_000);
        assert!(Json::parse(&deep).is_err());
        let ok = "[".repeat(10) + &"]".repeat(10);
        assert!(Json::parse(&ok).is_ok());
    }

    #[test]
    fn errors() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("\"abc").is_err());
        assert!(Json::parse("1 2").is_err());
    }
}
