//! NagiScript lexer: `.ngs` / `.ngsx` 共通の字句解析。
//! 文字列 → トークン列。コメント (//, /* */)、文字列リテラル、
//! 数値リテラル (10進/16進、サフィックス付き)、演算子を処理する。

use ngs_ast::{Span, Token, TokenKind};

#[derive(Debug, Clone)]
pub struct LexError {
    pub msg: String,
    pub span: Span,
}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lx = Lexer {
        src: src.as_bytes(),
        pos: 0,
        line: 1,
        col: 1,
    };
    let mut toks = Vec::new();
    loop {
        let t = lx.next_token()?;
        let eof = matches!(t.kind, TokenKind::Eof);
        toks.push(t);
        if eof {
            break;
        }
    }
    Ok(toks)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn make(&self, kind: TokenKind, start: usize) -> Token {
        Token {
            kind,
            span: Span::new(start, self.pos),
        }
    }

    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.bump();
                }
                Some(b'/') if self.peek2() == Some(b'/') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some(b'/') if self.peek2() == Some(b'*') => {
                    let start = self.pos;
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            None => {
                                return Err(LexError {
                                    msg: "unterminated block comment".into(),
                                    span: Span::new(start, self.pos),
                                })
                            }
                            Some(b'*') if self.peek2() == Some(b'/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let start = self.pos;
        let c = match self.peek() {
            None => return Ok(self.make(TokenKind::Eof, start)),
            Some(c) => c,
        };

        // identifiers / keywords
        if c.is_ascii_alphabetic() || c == b'_' {
            while let Some(c) = self.peek() {
                if c.is_ascii_alphanumeric() || c == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
            let text = &self.src[start..self.pos];
            // f"..." 補間文字列: 識別子 `f` の直後に `"` が続く場合のみ
            if text == b"f" && self.src.get(self.pos) == Some(&b'"') {
                return self.lex_fstring(start);
            }
            let kind = keyword_or_ident(text);
            return Ok(self.make(kind, start));
        }

        // numbers (with optional type suffix like 1u32, 2.5f32)
        if c.is_ascii_digit() {
            return self.lex_number(start);
        }

        // strings
        if c == b'"' {
            return self.lex_string(start);
        }

        // operators
        let two = |a: u8, b: u8| self.peek() == Some(a) && self.peek2() == Some(b);
        macro_rules! two_tok {
            ($k:expr) => {{
                self.bump();
                self.bump();
                return Ok(self.make($k, start));
            }};
        }
        macro_rules! one_tok {
            ($k:expr) => {{
                self.bump();
                return Ok(self.make($k, start));
            }};
        }

        if two(b'=', b'=') { two_tok!(TokenKind::EqEq); }
        if two(b'!', b'=') { two_tok!(TokenKind::NotEq); }
        if two(b'<', b'=') { two_tok!(TokenKind::Le); }
        if two(b'>', b'=') { two_tok!(TokenKind::Ge); }
        if two(b'&', b'&') { two_tok!(TokenKind::AndAnd); } // 論理AND &&
        if two(b'|', b'|') { two_tok!(TokenKind::OrOr); }
        if two(b'-', b'>') { two_tok!(TokenKind::Arrow); }
        if two(b'=', b'>') { two_tok!(TokenKind::FatArrow); }
        if two(b'.', b'.') { two_tok!(TokenKind::DotDot); }
        if two(b'+', b'=') { two_tok!(TokenKind::PlusAssign); }
        if two(b'-', b'=') { two_tok!(TokenKind::MinusAssign); }
        if two(b'*', b'=') { two_tok!(TokenKind::StarAssign); }
        if two(b'/', b'=') { two_tok!(TokenKind::SlashAssign); }
        if two(b'%', b'=') { two_tok!(TokenKind::PercentAssign); }

        match c {
            b'+' => one_tok!(TokenKind::Plus),
            b'-' => one_tok!(TokenKind::Minus),
            b'*' => one_tok!(TokenKind::Star),
            b'/' => one_tok!(TokenKind::Slash),
            b'%' => one_tok!(TokenKind::Percent),
            b'!' => one_tok!(TokenKind::Bang),
            b'<' => one_tok!(TokenKind::Lt),
            b'>' => one_tok!(TokenKind::Gt),
            b'=' => one_tok!(TokenKind::Assign),
            b'(' => one_tok!(TokenKind::LParen),
            b')' => one_tok!(TokenKind::RParen),
            b'{' => one_tok!(TokenKind::LBrace),
            b'}' => one_tok!(TokenKind::RBrace),
            b'[' => one_tok!(TokenKind::LBracket),
            b']' => one_tok!(TokenKind::RBracket),
            b',' => one_tok!(TokenKind::Comma),
            b':' => one_tok!(TokenKind::Colon),
            b';' => one_tok!(TokenKind::Semi),
            b'.' => one_tok!(TokenKind::Dot),
            b'?' => one_tok!(TokenKind::Question),
            b'@' => one_tok!(TokenKind::At),
            b'&' => one_tok!(TokenKind::Amp), // アドレス取得 & (単一)
            b'|' => one_tok!(TokenKind::Pipe),
            other => {
                self.bump();
                Err(LexError {
                    msg: format!("unexpected character '{}'", other as char),
                    span: Span::new(start, self.pos),
                })
            }
        }
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, LexError> {
        let mut is_float = false;

        if self.peek() == Some(b'0') && matches!(self.peek2(), Some(b'x')) {
            self.bump();
            self.bump();
            let hex_start = self.pos;
            while let Some(c) = self.peek() {
                if c.is_ascii_hexdigit() {
                    self.bump();
                } else {
                    break;
                }
            }
            let text = std::str::from_utf8(&self.src[hex_start..self.pos]).unwrap();
            if text.is_empty() {
                return Err(LexError {
                    msg: "invalid hex literal".into(),
                    span: Span::new(start, self.pos),
                });
            }
            let v = u64::from_str_radix(text, 16).map_err(|_| LexError {
                msg: "invalid hex literal".into(),
                span: Span::new(start, self.pos),
            })?;
            return Ok(self.make(TokenKind::IntLit(v), start));
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.bump();
            } else {
                break;
            }
        }
        // fraction
        if self.peek() == Some(b'.')
            && self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false)
        {
            is_float = true;
            self.bump(); // .
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        // exponent
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            let save = (self.pos, self.line, self.col);
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.bump();
            }
            if self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                is_float = true;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        self.bump();
                    } else {
                        break;
                    }
                }
            } else {
                self.pos = save.0;
                self.line = save.1;
                self.col = save.2;
            }
        }

        // optional suffix (u32, i64, f32, ...) — validated at sema/type level
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() || c == b'_' || c.is_ascii_digit() {
                self.bump();
            } else {
                break;
            }
        }

        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        if is_float {
            // strip suffix for parse; suffix checked by sema when used
            let (digits, _suf) = split_numeric_suffix(text);
            let v: f64 = digits.parse().map_err(|_| LexError {
                msg: format!("invalid float literal `{text}`"),
                span: Span::new(start, self.pos),
            })?;
            Ok(self.make(TokenKind::FloatLit(v), start))
        } else {
            let (digits, suf) = split_numeric_suffix(text);
            if !suf.is_empty()
                && !matches!(suf, "i8"|"i16"|"i32"|"i64"|"u8"|"u16"|"u32"|"u64"|"usize"|"isize")
            {
                return Err(LexError {
                    msg: format!("unknown integer suffix `{}`", suf),
                    span: Span::new(start, self.pos),
                });
            }
            let v: u64 = digits.parse().map_err(|_| LexError {
                msg: format!("invalid integer literal `{text}`"),
                span: Span::new(start, self.pos),
            })?;
            Ok(self.make(TokenKind::IntLit(v), start))
        }
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, LexError> {
        self.bump(); // opening "
        let mut out = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(LexError {
                        msg: "unterminated string literal".into(),
                        span: Span::new(start, self.pos),
                    })
                }
                Some(b'"') => break,
                Some(b'\n') => {
                    return Err(LexError {
                        msg: "newline in string literal".into(),
                        span: Span::new(start, self.pos),
                    })
                }
                Some(b'\\') => match self.bump() {
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'r') => out.push('\r'),
                    Some(b'0') => out.push('\0'),
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(other) => {
                        return Err(LexError {
                            msg: format!("unknown escape sequence `\\{}`", other as char),
                            span: Span::new(self.pos - 2, self.pos),
                        })
                    }
                    None => {
                        return Err(LexError {
                            msg: "unterminated escape".into(),
                            span: Span::new(start, self.pos),
                        })
                    }
                },
                Some(c) => {
                    // UTF-8 マルチバイトはそのまま透過させる
                    if c < 0x80 {
                        out.push(c as char);
                    } else {
                        // 先頭バイトからUTF-8文字を再構成
                        let len = utf8_len(c);
                        let mut bytes = vec![c];
                        for _ in 1..len {
                            if let Some(b) = self.bump() {
                                bytes.push(b);
                            }
                        }
                        if let Ok(s) = std::str::from_utf8(&bytes) {
                            out.push_str(s);
                        }
                    }
                }
            }
        }
        Ok(self.make(TokenKind::StrLit(out), start))
    }

    /// `f"..."` 文字列補間。`f"` が消費済み（self.pos は `"` の直後）の状態で呼ばれる。
    /// `{expr}` を式セグメント（ソース範囲）に、それ以外をテキストセグメントに分解する。
    /// `\{` / `\}` はリテラルな `{` / `}` としてテキストに含まれる（補間されない）。
    fn lex_fstring(&mut self, start: usize) -> Result<Token, LexError> {
        self.bump(); // opening "
        let mut segs: Vec<ngs_ast::FStrSeg> = Vec::new();
        let mut cur_text = String::new();
        let mut expr_start: Option<usize> = None; // { の直後位置
        let mut depth: u32 = 0;

        let flush_text = |text: &mut String, segs: &mut Vec<ngs_ast::FStrSeg>| {
            if !text.is_empty() {
                segs.push(ngs_ast::FStrSeg::Text(std::mem::take(text)));
            }
        };

        loop {
            let c = match self.peek() {
                None => {
                    return Err(LexError {
                        msg: "unterminated string literal".into(),
                        span: Span::new(start, self.pos),
                    })
                }
                Some(c) => c,
            };
            match c {
                b'"' => {
                    if expr_start.is_some() {
                        return Err(LexError {
                            msg: "expected `}` in string interpolation".into(),
                            span: Span::new(start, self.pos),
                        });
                    }
                    self.bump();
                    flush_text(&mut cur_text, &mut segs);
                    return Ok(self.make(TokenKind::FStr(segs), start));
                }
                b'\n' => {
                    return Err(LexError {
                        msg: "newline in string literal".into(),
                        span: Span::new(start, self.pos),
                    })
                }
                b'\\' => {
                    if expr_start.is_some() {
                        return Err(LexError {
                            msg: "escape inside interpolation is not allowed".into(),
                            span: Span::new(start, self.pos),
                        });
                    }
                    let e = self.bump(); // backslash
                    let _ = e;
                    match self.bump() {
                        Some(b'n') => cur_text.push('\n'),
                        Some(b't') => cur_text.push('\t'),
                        Some(b'r') => cur_text.push('\r'),
                        Some(b'0') => cur_text.push('\0'),
                        Some(b'"') => cur_text.push('"'),
                        Some(b'\\') => cur_text.push('\\'),
                        Some(b'{') => cur_text.push('{'),
                        Some(b'}') => cur_text.push('}'),
                        Some(other) => {
                            return Err(LexError {
                                msg: format!("unknown escape sequence `\\{}`", other as char),
                                span: Span::new(self.pos - 2, self.pos),
                            })
                        }
                        None => {
                            return Err(LexError {
                                msg: "unterminated escape".into(),
                                span: Span::new(start, self.pos),
                            })
                        }
                    }
                }
                b'{' => match expr_start {
                    None => {
                        flush_text(&mut cur_text, &mut segs);
                        expr_start = Some(self.pos + 1);
                        depth = 0;
                        self.bump();
                    }
                    Some(_) => {
                        depth += 1;
                        self.consume_utf8();
                    }
                },
                b'}' => match expr_start {
                    Some(_) if depth == 0 => {
                        // 式の終端: expr_start..(pos) が式本体
                        let hi = self.pos;
                        let seg = ngs_ast::FStrSeg::Expr(Span::new(expr_start.unwrap(), hi));
                        segs.push(seg);
                        expr_start = None;
                        self.bump();
                    }
                    Some(_) => {
                        depth -= 1;
                        self.consume_utf8();
                    }
                    None => {
                        cur_text.push('}');
                        self.bump();
                    }
                },
                _ => {
                    if let Some(es) = expr_start {
                        // 式内部: 文字列リテラル内の `{`/`}` は数えない（簡易）
                        let _ = es;
                        self.consume_utf8();
                    } else {
                        self.consume_utf8_text(&mut cur_text);
                    }
                }
            }
        }
    }

    /// 式セグメント内の1文字（UTF-8 マルチバイトをまとめて）を消費する。
    fn consume_utf8(&mut self) {
        let c = self.bump();
        if let Some(c) = c {
            if c >= 0x80 {
                let len = utf8_len(c);
                for _ in 1..len {
                    self.bump();
                }
            }
        }
    }

    /// テキストセグメントに1文字（UTF-8 マルチバイトをまとめて）を追加し消費する。
    fn consume_utf8_text(&mut self, out: &mut String) {
        let c = self.bump();
        if let Some(c) = c {
            if c < 0x80 {
                out.push(c as char);
            } else {
                let len = utf8_len(c);
                let mut bytes = vec![c];
                for _ in 1..len {
                    if let Some(b) = self.bump() {
                        bytes.push(b);
                    }
                }
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    out.push_str(s);
                }
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first >= 0xF0 {
        4
    } else if first >= 0xE0 {
        3
    } else {
        2
    }
}

fn split_numeric_suffix(text: &str) -> (&str, &str) {
    // 末尾のアルファベット列をサフィックスとして分離（123abc → "123","abc"）
    let cut = text
        .rfind(|c: char| !c.is_ascii_alphabetic())
        .map(|i| i + 1)
        .unwrap_or(0);
    (&text[..cut], &text[cut..])
}

fn keyword_or_ident(text: &[u8]) -> TokenKind {
    use TokenKind::*;
    match text {
        b"fn" => KwFn,
        b"val" => KwLet,
        b"var" => KwVar,
        b"if" => KwIf,
        b"else" => KwElse,
        b"for" => KwFor,
        b"while" => KwWhile,
        b"match" => KwMatch,
        b"struct" => KwStruct,
        b"enum" => KwEnum,
        b"unsafe" => KwUnsafe,
        b"extern" => KwExtern,
        b"export" => KwExport,
        b"return" => KwReturn,
        b"impl" => KwImpl,
        b"in" => KwIn,
        b"as" => KwAs,
        b"break" => KwBreak,
        b"continue" => KwContinue,
        b"true" => KwTrue,
        b"false" => KwFalse,
        b"null" => KwNull,
        _ => Ident(String::from_utf8_lossy(text).into_owned()),
    }
}
