use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}
impl Json {
    pub fn get(&self, k: &str) -> &Json {
        if let Self::Obj(o) = self {
            o.get(k).unwrap_or(&Self::Null)
        } else {
            &Self::Null
        }
    }
    pub fn str(&self) -> &str {
        if let Self::Str(s) = self { s } else { "" }
    }
    pub fn num(&self) -> i64 {
        if let Self::Num(n) = self { *n } else { 0 }
    }
    pub fn arr(&self) -> &[Json] {
        if let Self::Arr(a) = self { a } else { &[] }
    }
    pub fn encode(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) => n.to_string(),
            Self::Str(s) => quote(s),
            Self::Arr(a) => format!(
                "[{}]",
                a.iter().map(Self::encode).collect::<Vec<_>>().join(",")
            ),
            Self::Obj(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}:{}", quote(k), v.encode()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser {
        c: s.chars().collect(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.c.len() {
        return Err("Лишние данные JSON".into());
    }
    Ok(v)
}
struct Parser {
    c: Vec<char>,
    i: usize,
}
impl Parser {
    fn ws(&mut self) {
        while self.c.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1
        }
    }
    fn take(&mut self) -> Result<char, String> {
        let c = self
            .c
            .get(self.i)
            .copied()
            .ok_or("Неожиданный конец JSON")?;
        self.i += 1;
        Ok(c)
    }
    fn string(&mut self) -> Result<String, String> {
        if self.take()? != '"' {
            return Err("Ожидается строка".into());
        }
        let mut s = String::new();
        loop {
            match self.take()? {
                '"' => return Ok(s),
                '\\' => {
                    let c = self.take()?;
                    s.push(match c {
                        '"' | '\\' | '/' => c,
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let mut n = 0;
                            for _ in 0..4 {
                                n = n * 16 + self.take()?.to_digit(16).ok_or("Unicode escape")?;
                            }
                            char::from_u32(n).ok_or("Unicode codepoint")?
                        }
                        _ => return Err("Некорректный escape".into()),
                    })
                }
                c if c < ' ' => return Err("Управляющий символ".into()),
                c => s.push(c),
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 32 {
            return Err("JSON слишком глубокий".into());
        }
        self.ws();
        match self.c.get(self.i).copied() {
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('{') => {
                self.i += 1;
                let mut o = BTreeMap::new();
                self.ws();
                if self.c.get(self.i) == Some(&'}') {
                    self.i += 1;
                    return Ok(Json::Obj(o));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if self.take()? != ':' {
                        return Err("Ожидается :".into());
                    }
                    let v = self.value(depth + 1)?;
                    o.insert(k, v);
                    self.ws();
                    match self.take()? {
                        '}' => break,
                        ',' => {}
                        _ => return Err("Ожидается , или }".into()),
                    }
                }
                Ok(Json::Obj(o))
            }
            Some('[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.c.get(self.i) == Some(&']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.take()? {
                        ']' => break,
                        ',' => {}
                        _ => return Err("Ожидается , или ]".into()),
                    }
                }
                Ok(Json::Arr(a))
            }
            Some('-' | '0'..='9') => {
                let start = self.i;
                self.i += 1;
                while self.c.get(self.i).is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1
                }
                let n: String = self.c[start..self.i].iter().collect();
                let digits = n.strip_prefix('-').unwrap_or(&n);
                if digits.len() > 1 && digits.starts_with('0') {
                    return Err("Ведущие нули запрещены".into());
                }
                Ok(Json::Num(n.parse().map_err(|_| "Число вне диапазона")?))
            }
            _ => {
                for (s, v) in [
                    ("true", Json::Bool(true)),
                    ("false", Json::Bool(false)),
                    ("null", Json::Null),
                ] {
                    let chars: Vec<_> = s.chars().collect();
                    if self.c.get(self.i..self.i + chars.len()) == Some(chars.as_slice()) {
                        self.i += chars.len();
                        return Ok(v);
                    }
                }
                Err("Некорректный JSON".into())
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = r#"{"name":"Русский\nтекст","steps":[1,true,null]}"#;
        let v = parse(s).unwrap();
        assert_eq!(parse(&v.encode()).unwrap(), v);
    }
    #[test]
    fn reject() {
        for s in ["{", "[1,]", "true false", "\"\\q\""] {
            assert!(parse(s).is_err())
        }
    }
}
