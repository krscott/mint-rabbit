//! A reader for the small subset of C the decomp uses for asset data:
//! top-level `name = initializer;` declarations whose initializers are
//! brace lists of constant expressions and macro calls.
//!
//! The only directive accepted is a plain `#include "file"`; any other is a
//! syntax error. Included `.inc` files are spliced in, since that is how the
//! decomp pulls extracted asset data into its sources; other includes are
//! headers the reader does not need.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}:{line}: {message}")]
    Syntax {
        path: PathBuf,
        line: u32,
        message: String,
    },
    #[error("{path}:{line}: cannot find included file {name:?}")]
    MissingInclude {
        path: PathBuf,
        line: u32,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Punct(&'static str),
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    file: u32,
    line: u32,
}

const PUNCTS: &[&str] = &[
    "<<", ">>", "->", "{", "}", "(", ")", "[", "]", ",", ";", "=", "+", "-", "*", "/", "%", "|",
    "&", "^", "~", "!", "<", ">", "?", ":", ".",
];

/// A constant expression from an initializer.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Call(String, Vec<Expr>),
    Unary(char, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    AddrOf(Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Sizeof(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    Xor,
    And,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Int(v) => write!(f, "{v}"),
            // Debug keeps a decimal point on whole numbers.
            Expr::Float(v) => write!(f, "{v:?}"),
            // The text keeps its source escapes.
            Expr::Str(s) => write!(f, "\"{s}\""),
            Expr::Ident(s) => f.write_str(s),
            Expr::Call(name, args) => {
                write!(f, "{name}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str(")")
            }
            // A space keeps `- -x` from reading as `--x`.
            Expr::Unary(op, e) if matches!(**e, Expr::Unary(..)) => write!(f, "{op} {e}"),
            Expr::Unary(op, e) => write!(f, "{op}{e}"),
            Expr::Binary(op, l, r) => {
                let op = match op {
                    BinOp::Or => "|",
                    BinOp::Xor => "^",
                    BinOp::And => "&",
                    BinOp::Shl => "<<",
                    BinOp::Shr => ">>",
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Rem => "%",
                };
                write!(f, "({l} {op} {r})")
            }
            Expr::AddrOf(e) => write!(f, "&{e}"),
            Expr::Index(a, i) => write!(f, "{a}[{i}]"),
            Expr::Sizeof(t) => write!(f, "sizeof({t})"),
        }
    }
}

impl Expr {
    /// Evaluates an integer constant expression; identifiers have no value.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        Some(match self {
            Expr::Int(v) => *v,
            // Whole floats appear in address arithmetic, e.g.
            // `sizeof(Mtx) * 10.0` in rhn_1.
            Expr::Float(v) if v.fract() == 0.0 && v.abs() < 1e15 => *v as i64,
            Expr::Sizeof(ty) => size_of_type(ty)?,
            Expr::Unary('-', e) => e.as_int()?.checked_neg()?,
            Expr::Unary('~', e) => !e.as_int()?,
            Expr::Binary(op, l, r) => {
                let (l, r) = (l.as_int()?, r.as_int()?);
                match op {
                    BinOp::Or => l | r,
                    BinOp::Xor => l ^ r,
                    BinOp::And => l & r,
                    BinOp::Shl => l.checked_shl(u32::try_from(r).ok()?)?,
                    BinOp::Shr => l.checked_shr(u32::try_from(r).ok()?)?,
                    BinOp::Add => l.checked_add(r)?,
                    BinOp::Sub => l.checked_sub(r)?,
                    BinOp::Mul => l.checked_mul(r)?,
                    BinOp::Div => l.checked_div(r)?,
                    BinOp::Rem => l.checked_rem(r)?,
                }
            }
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_ident(&self) -> Option<&str> {
        match self {
            Expr::Ident(s) => Some(s),
            _ => None,
        }
    }

    /// Splits `symbol + constant` (in either order) into its parts; a bare
    /// symbol has offset 0.
    #[must_use]
    pub fn as_symbol_offset(&self) -> Option<(&str, i64)> {
        match self {
            Expr::Ident(s) => Some((s, 0)),
            Expr::Binary(BinOp::Add, l, r) => match (l.as_ident(), r.as_ident()) {
                (Some(s), None) => Some((s, r.as_int()?)),
                (None, Some(s)) => Some((s, l.as_int()?)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Sizes of the decomp types that appear in `sizeof` within asset data.
fn size_of_type(ty: &str) -> Option<i64> {
    match ty {
        "Mtx" => Some(64),
        "Vtx" => Some(16),
        _ => None,
    }
}

/// An initializer: a brace list or a single expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Init {
    List(Vec<Init>),
    Expr(Expr),
}

/// Prints the initializer back as C.
impl fmt::Display for Init {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Init::Expr(e) => write!(f, "{e}"),
            Init::List(items) => {
                f.write_str("{ ")?;
                for item in items {
                    write!(f, "{item}, ")?;
                }
                f.write_str("}")
            }
        }
    }
}

impl Init {
    #[must_use]
    pub fn as_list(&self) -> Option<&[Init]> {
        match self {
            Init::List(items) => Some(items),
            Init::Expr(_) => None,
        }
    }

    #[must_use]
    pub fn as_expr(&self) -> Option<&Expr> {
        match self {
            Init::Expr(e) => Some(e),
            Init::List(_) => None,
        }
    }

    /// The integers of a flat list, or `None` if any item is not an integer
    /// constant.
    #[must_use]
    pub fn as_ints(&self) -> Option<Vec<i64>> {
        self.as_list()?
            .iter()
            .map(|i| i.as_expr()?.as_int())
            .collect()
    }
}

/// One `name = initializer;` declaration.
#[derive(Clone, Debug)]
pub struct Decl {
    pub name: String,
    pub init: Init,
    /// Declared `static`, so other files can't refer to it.
    pub is_static: bool,
}

/// The declarations of one C file, in source order.
#[derive(Debug, Default)]
pub struct CFile {
    pub path: PathBuf,
    pub decls: Vec<Decl>,
    by_name: HashMap<String, usize>,
}

impl CFile {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Decl> {
        self.by_name.get(name).map(|&i| &self.decls[i])
    }

    /// Reads a C file, splicing in `.inc` files found next to it or in one of
    /// `include_dirs`.
    pub fn read(path: &Path, include_dirs: &[PathBuf]) -> Result<Self, CError> {
        let mut files = Vec::new();
        let mut tokens = Vec::new();
        tokenize_file(path, include_dirs, &mut files, &mut tokens)?;
        Self::parse(path, &files, &tokens)
    }

    /// Parses C source text that includes no files.
    pub fn parse_str(path: &Path, text: &str) -> Result<Self, CError> {
        let mut files = vec![path.to_owned()];
        let mut tokens = Vec::new();
        tokenize(text, 0, &mut files, &[], &mut tokens)?;
        Self::parse(path, &files, &tokens)
    }

    fn parse(path: &Path, files: &[PathBuf], tokens: &[Token]) -> Result<Self, CError> {
        let decls = Parser {
            tokens,
            pos: 0,
            files,
        }
        .decls()?;
        let by_name = decls
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name.clone(), i))
            .collect();
        Ok(Self {
            path: path.to_owned(),
            decls,
            by_name,
        })
    }
}

fn tokenize_file(
    path: &Path,
    include_dirs: &[PathBuf],
    files: &mut Vec<PathBuf>,
    out: &mut Vec<Token>,
) -> Result<(), CError> {
    let text = std::fs::read_to_string(path).map_err(|source| CError::Io {
        path: path.to_owned(),
        source,
    })?;
    let file = files.len() as u32;
    files.push(path.to_owned());
    let mut dirs: Vec<PathBuf> = path.parent().map(Path::to_owned).into_iter().collect();
    dirs.extend_from_slice(include_dirs);
    tokenize(&text, file, files, &dirs, out)
}

fn tokenize(
    text: &str,
    file: u32,
    files: &mut Vec<PathBuf>,
    include_dirs: &[PathBuf],
    out: &mut Vec<Token>,
) -> Result<(), CError> {
    let path = files[file as usize].clone();
    let syntax = |line: u32, message: String| CError::Syntax {
        path: path.clone(),
        line,
        message,
    };
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut line = 1u32;
    let mut at_line_start = true;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            at_line_start = true;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'#' && at_line_start {
            let end = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            let name = include_name(&text[i + 1..end]).ok_or_else(|| {
                syntax(
                    line,
                    format!(
                        "unsupported directive {:?}; asset sources may only use #include \"file\"",
                        text[i..end].trim()
                    ),
                )
            })?;
            // Only extracted asset data (.inc) is spliced in; headers declare
            // types and macros the reader does not need.
            if Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("inc"))
            {
                let found = include_dirs
                    .iter()
                    .map(|d| d.join(name))
                    .find(|p| p.is_file())
                    .ok_or_else(|| CError::MissingInclude {
                        path: path.clone(),
                        line,
                        name: name.to_owned(),
                    })?;
                tokenize_file(&found, include_dirs, files, out)?;
            }
            i = end;
            continue;
        }
        at_line_start = false;
        if text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if text[i..].starts_with("/*") {
            let end = text[i + 2..]
                .find("*/")
                .ok_or_else(|| syntax(line, "unterminated comment".into()))?;
            line += text[i..i + 2 + end].matches('\n').count() as u32;
            i += end + 4;
            continue;
        }
        let tok = if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            Tok::Ident(text[start..i].to_owned())
        } else if c.is_ascii_digit()
            || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'.'
                    || ((bytes[i] == b'+' || bytes[i] == b'-')
                        && matches!(bytes[i - 1], b'e' | b'E')
                        && !text[start..i].starts_with("0x")))
            {
                i += 1;
            }
            parse_number(&text[start..i])
                .ok_or_else(|| syntax(line, format!("bad number {:?}", &text[start..i])))?
        } else if c == b'"' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            let s = text[start..i.min(bytes.len())].to_owned();
            i += 1;
            Tok::Str(s)
        } else if let Some(p) = PUNCTS.iter().find(|p| text[i..].starts_with(**p)) {
            i += p.len();
            Tok::Punct(p)
        } else {
            return Err(syntax(
                line,
                format!("unexpected character {:?}", c as char),
            ));
        };
        out.push(Token { tok, file, line });
    }
    Ok(())
}

/// The file named by a directive line (the text after `#`) of exactly the
/// form `include "file"`, or `None` for anything else. Asset sources use no
/// other directives, so the reader refuses them rather than emulating the
/// preprocessor.
fn include_name(directive: &str) -> Option<&str> {
    let name = directive
        .trim()
        .strip_prefix("include")?
        .trim_start()
        .strip_prefix('"')?
        .strip_suffix('"')?;
    (!name.contains('"')).then_some(name)
}

fn parse_number(s: &str) -> Option<Tok> {
    let lower = s.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        let digits = hex.trim_end_matches(['u', 'l']);
        return i64::from_str_radix(digits, 16).ok().map(Tok::Int);
    }
    let is_float = lower.contains('.') || lower.contains('e') || lower.ends_with('f');
    if is_float {
        return lower.trim_end_matches('f').parse().ok().map(Tok::Float);
    }
    let digits = lower.trim_end_matches(['u', 'l']);
    if digits.len() > 1 && digits.starts_with('0') {
        return i64::from_str_radix(&digits[1..], 8).ok().map(Tok::Int);
    }
    digits.parse().ok().map(Tok::Int)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    files: &'a [PathBuf],
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> CError {
        let tok = self.tokens.get(self.pos).or(self.tokens.last());
        CError::Syntax {
            path: tok.map_or_else(PathBuf::new, |t| self.files[t.file as usize].clone()),
            line: tok.map_or(0, |t| t.line),
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(q)) if *q == p)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<(), CError> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(self.error(format!("expected {p:?}, found {:?}", self.peek())))
        }
    }

    /// Scans top-level tokens, collecting each `name ... = init;`.
    fn decls(&mut self) -> Result<Vec<Decl>, CError> {
        let mut decls = Vec::new();
        let mut statement_start = self.pos;
        let mut depth = 0i32;
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Punct("(" | "[" | "{") => depth += 1,
                Tok::Punct(")" | "]" | "}") => depth -= 1,
                Tok::Punct(";") if depth == 0 => statement_start = self.pos + 1,
                Tok::Punct("=") if depth == 0 => {
                    let (name, is_static) = self.declared_name(statement_start)?;
                    self.pos += 1;
                    let init = self.init()?;
                    self.expect(";")?;
                    decls.push(Decl {
                        name,
                        init,
                        is_static,
                    });
                    statement_start = self.pos;
                    continue;
                }
                _ => {}
            }
            self.pos += 1;
        }
        Ok(decls)
    }

    /// The last identifier at bracket depth 0 between `start` and the `=`,
    /// which skips attribute groups and array suffixes, and whether the
    /// declaration is `static`.
    fn declared_name(&self, start: usize) -> Result<(String, bool), CError> {
        let mut depth = 0i32;
        let mut name = None;
        let mut is_static = false;
        for t in &self.tokens[start..self.pos] {
            match &t.tok {
                Tok::Punct("(" | "[") => depth += 1,
                Tok::Punct(")" | "]") => depth -= 1,
                Tok::Ident(s) if depth == 0 && s == "static" => is_static = true,
                Tok::Ident(s) if depth == 0 && s != "__attribute__" => name = Some(s.clone()),
                _ => {}
            }
        }
        let name = name.ok_or_else(|| self.error("declaration without a name"))?;
        Ok((name, is_static))
    }

    fn init(&mut self) -> Result<Init, CError> {
        if !self.eat("{") {
            return Ok(Init::Expr(self.expr(0)?));
        }
        let mut items = Vec::new();
        while !self.eat("}") {
            items.push(self.init()?);
            if !self.eat(",") {
                self.expect("}")?;
                break;
            }
        }
        Ok(Init::List(items))
    }

    fn binop(&self) -> Option<(BinOp, u8)> {
        let Some(Tok::Punct(p)) = self.peek() else {
            return None;
        };
        Some(match *p {
            "|" => (BinOp::Or, 1),
            "^" => (BinOp::Xor, 2),
            "&" => (BinOp::And, 3),
            "<<" => (BinOp::Shl, 4),
            ">>" => (BinOp::Shr, 4),
            "+" => (BinOp::Add, 5),
            "-" => (BinOp::Sub, 5),
            "*" => (BinOp::Mul, 6),
            "/" => (BinOp::Div, 6),
            "%" => (BinOp::Rem, 6),
            _ => return None,
        })
    }

    fn expr(&mut self, min_prec: u8) -> Result<Expr, CError> {
        let mut lhs = self.unary()?;
        while let Some((op, prec)) = self.binop() {
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let rhs = self.expr(prec + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, CError> {
        for op in ['-', '~', '+'] {
            if self.eat(&op.to_string()) {
                let e = self.unary()?;
                return Ok(if op == '+' {
                    e
                } else {
                    Expr::Unary(op, Box::new(e))
                });
            }
        }
        if self.eat("&") {
            return Ok(Expr::AddrOf(Box::new(self.unary()?)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, CError> {
        let mut e = self.primary()?;
        while self.eat("[") {
            let idx = self.expr(0)?;
            self.expect("]")?;
            e = Expr::Index(Box::new(e), Box::new(idx));
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, CError> {
        let Some(tok) = self.peek().cloned() else {
            return Err(self.error("unexpected end of file"));
        };
        self.pos += 1;
        match tok {
            Tok::Int(v) => Ok(Expr::Int(v)),
            Tok::Float(v) => Ok(Expr::Float(v)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Punct("(") => {
                let e = self.expr(0)?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident(name) if name == "sizeof" => {
                self.expect("(")?;
                let Some(Tok::Ident(ty)) = self.peek().cloned() else {
                    return Err(self.error("expected a type name in sizeof"));
                };
                self.pos += 1;
                self.expect(")")?;
                Ok(Expr::Sizeof(ty))
            }
            Tok::Ident(name) => {
                if !self.eat("(") {
                    return Ok(Expr::Ident(name));
                }
                let mut args = Vec::new();
                while !self.eat(")") {
                    args.push(self.expr(0)?);
                    if !self.eat(",") {
                        self.expect(")")?;
                        break;
                    }
                }
                Ok(Expr::Call(name, args))
            }
            other @ Tok::Punct(_) => {
                self.pos -= 1;
                Err(self.error(format!("unexpected {other:?}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializers_print_back_as_equivalent_c() {
        let file = parse(r#"int x[] = { 1.0 / 2, - -3, "a\n", 0.008999999612569809, -1 };"#);
        let printed = file.get("x").unwrap().init.to_string();
        assert_eq!(
            printed,
            r#"{ (1.0 / 2), - -3, "a\n", 0.008999999612569809, -1, }"#
        );
    }

    fn parse(text: &str) -> CFile {
        CFile::parse_str(Path::new("test.c"), text).unwrap()
    }

    #[test]
    fn reads_display_list_macros() {
        let f = parse(
            r#"
            #include "c_keyframe.h"
            static Gfx head_model[] = {
                gsSPMatrix(anime_6_model+sizeof(Mtx)*7, G_MTX_NOPUSH | G_MTX_LOAD),
                gsSPVertex(&cat_1_v[6], 13, 6), // trailing comment
                gsDPLoadTextureBlock_4b_Dolphin(anime_4_txt + 0x300, G_IM_FMT_CI, 16, 32, 15,
                    GX_CLAMP, GX_CLAMP, 0, 0),
                gsSPEndDisplayList(),
            };"#,
        );
        let dl = f.get("head_model").unwrap().init.as_list().unwrap();
        assert_eq!(dl.len(), 4);
        let Some(Expr::Call(name, args)) = dl[1].as_expr() else {
            panic!("{:?}", dl[1]);
        };
        assert_eq!(name, "gsSPVertex");
        assert_eq!(
            args[0],
            Expr::AddrOf(Box::new(Expr::Index(
                Box::new(Expr::Ident("cat_1_v".into())),
                Box::new(Expr::Int(6))
            )))
        );
        let Some(Expr::Call(_, args)) = dl[2].as_expr() else {
            panic!()
        };
        assert_eq!(args[0].as_symbol_offset(), Some(("anime_4_txt", 0x300)));
    }

    #[test]
    fn reads_attributed_arrays_and_floats() {
        let f = parse(
            "extern unsigned short pal[] __attribute__((aligned(32))) = { 0x7FFF, 12, -3 };
             float scale = 0.008999999612569809;
             /* block */ s16 t[] = { 1.0f, 2e3 };",
        );
        assert_eq!(
            f.get("pal").unwrap().init.as_ints(),
            Some(vec![0x7FFF, 12, -3])
        );
        assert_eq!(
            f.get("scale").unwrap().init.as_expr(),
            Some(&Expr::Float(0.008_999_999_612_569_809))
        );
        assert!(f.get("t").is_some());
    }

    #[test]
    fn refuses_directives_other_than_include() {
        for text in [
            "#if(0)\nint a = 1;\n#endif\n",
            "#/**/if 1\nint a = 1;\n",
            "#define X 1\n",
            "#include \"c_keyframe.h\" /* trailing */\n",
            "#\\\nif 0\n",
        ] {
            let err = CFile::parse_str(Path::new("t.c"), text).unwrap_err();
            assert!(
                err.to_string().contains("unsupported directive"),
                "{text:?}: {err}"
            );
        }
        assert!(CFile::parse_str(Path::new("t.c"), "# include \"c_keyframe.h\"\n").is_ok());
    }

    #[test]
    fn splices_inc_files() {
        let dir = std::env::temp_dir().join(format!("mint-ac-c-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/v.inc"), "{1, 2}, {3, 4},\n").unwrap();
        std::fs::write(
            dir.join("m.c"),
            "static Vtx v[] = {\n#include \"assets/v.inc\"\n};\n",
        )
        .unwrap();
        let f = CFile::read(&dir.join("m.c"), &[]).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let v = f.get("v").unwrap().init.as_list().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].as_ints(), Some(vec![3, 4]));
    }
}
