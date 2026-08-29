//! NagiScript 再帰下降パーサ。
//! トークン列 → AST。`.ngsx` では JSX 風マークアップ式を解析し、
//! 仕様 4.4 節の通り `createElement(tag, props, ...children)` 関数呼び出しへの
//! 糖衣構文として AST に展開する。

use ngs_ast::*;
use ngs_lexer::LexError;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub msg: String,
    pub span: Span,
}

#[derive(Debug)]
pub enum FrontError {
    Lex(LexError),
    Parse(ParseError),
}

impl std::fmt::Display for FrontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontError::Lex(e) => write!(f, "{}", e.msg),
            FrontError::Parse(e) => write!(f, "{}", e.msg),
        }
    }
}

pub struct Parser {
    src: String,
    toks: Vec<Token>,
    pos: usize,
    jsx: bool,
    /// 条件式位置 (if / match の被検査体) では `Ident { .. }` 構造体リテラルを禁止
    no_struct_depth: u32,
    /// 括弧のネスト深さ（addendum 3.4: 病的ネストへの静的上限）
    paren_depth: u32,
}

/// ソース文字列を直接パースする（path で .ngsx 判定）
pub fn parse_source(src: &str, path: &str) -> Result<File, FrontError> {
    let jsx = path.ends_with(".ngsx");
    let toks = ngs_lexer::lex(src).map_err(FrontError::Lex)?;
    let mut p = Parser { src: src.to_string(), toks, pos: 0, jsx, no_struct_depth: 0, paren_depth: 0 };
    let mut items = Vec::new();
    while !p.at_eof() {
        match p.parse_item() {
            Ok(it) => items.push(it),
            Err(e) => return Err(FrontError::Parse(e)),
        }
        p.eat(&TokenKind::Semi);
    }
    let mut f = File { items, jsx, path: path.to_string() };
    let _ = &mut f;
    Ok(f)
}

pub fn parse_tokens(toks: Vec<Token>, jsx: bool) -> Result<File, ParseError> {
    let mut p = Parser { src: String::new(), toks, pos: 0, jsx, no_struct_depth: 0, paren_depth: 0 };
    let mut items = Vec::new();
    while !p.at_eof() {
        items.push(p.parse_item()?);
        p.eat(&TokenKind::Semi);
    }
    Ok(File { items, jsx, path: String::new() })
}

impl Parser {
    // ------------------------------------------------------------------
    // token helpers
    // ------------------------------------------------------------------
    fn peek(&self) -> Token {
        self.toks[self.pos.min(self.toks.len() - 1)].clone()
    }
    fn peek_kind(&self) -> TokenKind {
        self.peek().kind
    }
    fn peek2_kind(&self) -> TokenKind {
        let i = (self.pos + 1).min(self.toks.len() - 1);
        self.toks[i].kind.clone()
    }
    fn bump(&mut self) -> Token {
        let t = self.peek();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn at(&self, k: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek_kind()) == std::mem::discriminant(k)
    }
    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.at(k) { self.bump(); true } else { false }
    }
    fn expect(&mut self, k: &TokenKind, what: &str) -> Result<Token, ParseError> {
        if self.at(k) { Ok(self.bump()) } else { Err(self.err(format!("expected {what}"))) }
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { msg: msg.into(), span: self.peek().span }
    }
    fn ident(&mut self) -> Result<(String, Span), ParseError> {
        match self.peek_kind() {
            TokenKind::Ident(s) => {
                let s = s.clone();
                let t = self.bump();
                Ok((s, t.span))
            }
            _ => Err(self.err("expected identifier")),
        }
    }

    // ------------------------------------------------------------------
    // items
    // ------------------------------------------------------------------
    fn parse_item(&mut self) -> Result<Item, ParseError> {
        let mut repr_c = false;
        while self.at(&TokenKind::At) {
            self.bump();
            let (attr, _) = self.ident()?;
            if attr == "repr" {
                self.expect(&TokenKind::LParen, "`(` after @repr")?;
                let (abi, _) = self.ident()?;
                if abi != "C" {
                    return Err(self.err("only @repr(C) is supported"));
                }
                self.expect(&TokenKind::RParen, "`)` after @repr(C)")?;
                repr_c = true;
            } else {
                return Err(self.err(format!("unknown attribute @{attr}")));
            }
        }
        match self.peek_kind() {
            TokenKind::KwFn | TokenKind::KwExtern | TokenKind::KwExport => {
                Ok(Item::Fn(self.parse_fn(repr_c)?))
            }
            TokenKind::KwStruct => Ok(Item::Struct(self.parse_struct(repr_c)?)),
            TokenKind::KwEnum => Ok(Item::Enum(self.parse_enum(repr_c)?)),
            TokenKind::KwImpl => Ok(Item::Impl(self.parse_impl()?)),
            _ => Err(self.err("expected item (fn / struct / enum / impl)")),
        }
    }

    fn parse_abi(&mut self) -> Result<String, ParseError> {
        match self.peek_kind() {
            TokenKind::StrLit(s) => {
                let t = self.bump();
                if let TokenKind::StrLit(v) = t.kind { Ok(v) } else { unreachable!() }
            }
            _ => Err(self.err("ABI string expected, e.g. \"C\"")),
        }
    }

    fn parse_type_params(&mut self) -> Result<Vec<String>, ParseError> {
        let mut out = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                let (n, _) = self.ident()?;
                out.push(n);
                if !self.eat(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::Gt, "`>` closing type parameters")?;
        }
        Ok(out)
    }

    fn parse_fn(&mut self, repr_c: bool) -> Result<FnDecl, ParseError> {
        if repr_c {
            return Err(self.err("@repr(C) applies only to struct/enum"));
        }
        let start = self.peek().span.lo;
        let mut extern_abi = None;
        let mut export_abi = None;
        if self.eat(&TokenKind::KwExtern) {
            extern_abi = Some(self.parse_abi()?);
        } else if self.eat(&TokenKind::KwExport) {
            export_abi = Some(self.parse_abi()?);
        }
        self.expect(&TokenKind::KwFn, "`fn`")?;
        let (name, name_span) = self.ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&TokenKind::LParen, "`(` after function name")?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                let (pname, psp) = self.ident()?;
                self.expect(&TokenKind::Colon, "`:` after parameter name")?;
                let ty = self.parse_type()?;
                params.push(Param { name: pname, ty, span: psp });
                if !self.eat(&TokenKind::Comma) { break; }
            }
        }
        let end_paren = self.expect(&TokenKind::RParen, "`)` after parameters")?;
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        if extern_abi.is_some() && export_abi.is_none() {
            // 宣言のみ
            self.eat(&TokenKind::Semi);
            let end = self.toks[self.pos - 1].span.hi;
            return Ok(FnDecl {
                name,
                name_span,
                type_params,
                params,
                ret,
                body: None,
                extern_abi,
                export_abi,
                span: Span::new(start, end.max(end_paren.span.hi)),
            });
        }
        if self.eat(&TokenKind::Semi) {
            let end = self.toks[self.pos - 1].span.hi;
            return Ok(FnDecl {
                name,
                name_span,
                type_params,
                params,
                ret,
                body: None,
                extern_abi,
                export_abi,
                span: Span::new(start, end),
            });
        }
        let body = self.parse_block()?;
        let end = body.span.hi;
        Ok(FnDecl {
            name,
            name_span,
            type_params,
            params,
            ret,
            body: Some(body),
            extern_abi,
            export_abi,
            span: Span::new(start, end),
        })
    }

    fn parse_struct(&mut self, repr_c: bool) -> Result<StructDecl, ParseError> {
        let start = self.peek().span.lo;
        self.expect(&TokenKind::KwStruct, "`struct`")?;
        let (name, name_span) = self.ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let (fname, fspan) = self.ident()?;
            self.expect(&TokenKind::Colon, "`:` after field name")?;
            let ty = self.parse_type()?;
            fields.push(FieldDef { name: fname, ty, span: fspan });
            self.eat(&TokenKind::Comma);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span.hi;
        Ok(StructDecl { name, name_span, type_params, fields, repr_c, span: Span::new(start, end) })
    }

    fn parse_enum(&mut self, repr_c: bool) -> Result<EnumDecl, ParseError> {
        let start = self.peek().span.lo;
        self.expect(&TokenKind::KwEnum, "`enum`")?;
        let (name, name_span) = self.ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let (vname, vspan) = self.ident()?;
            let mut payloads = Vec::new();
            if self.eat(&TokenKind::LParen) {
                loop {
                    payloads.push(self.parse_type()?);
                    if !self.eat(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen, "`)` after variant payload types")?;
            }
            variants.push(VariantDef { name: vname, payload_types: payloads, span: vspan });
            self.eat(&TokenKind::Comma);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span.hi;
        Ok(EnumDecl { name, name_span, type_params, variants, repr_c, span: Span::new(start, end) })
    }

    fn parse_impl(&mut self) -> Result<ImplBlock, ParseError> {
        let start = self.peek().span.lo;
        self.expect(&TokenKind::KwImpl, "`impl`")?;
        let (type_name, _) = self.ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            methods.push(self.parse_fn(false)?);
            self.eat(&TokenKind::Semi);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span.hi;
        Ok(ImplBlock { type_name, type_params, methods, span: Span::new(start, end) })
    }

    // ------------------------------------------------------------------
    // types
    // ------------------------------------------------------------------
    pub fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let start = self.peek().span.lo;
        // *T
        if self.eat(&TokenKind::Star) {
            let elem = Box::new(self.parse_type()?);
            let end = elem.span().hi;
            return Ok(TypeExpr::Ptr { elem, span: Span::new(start, end) });
        }
        // [T; N]
        if self.at(&TokenKind::LBracket) {
            self.bump();
            let elem = Box::new(self.parse_type()?);
            self.expect(&TokenKind::Semi, "`;` in array type")?;
            match self.peek_kind() {
                TokenKind::IntLit(n) => {
                    let len = n;
                    self.bump();
                    let endb = self.expect(&TokenKind::RBracket, "`]` in array type")?;
                    let end = endb.span.hi;
                    return Ok(TypeExpr::Array { elem, len, span: Span::new(start, end) });
                }
                _ => return Err(self.err("array length must be an integer literal")),
            }
        }
        let (name, nspan) = match self.peek_kind() {
            TokenKind::Ident(s) => {
                let s = s.clone();
                let t = self.bump();
                (s, t.span)
            }
            _ => return Err(self.err("expected type name")),
        };
        let mut args = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                args.push(self.parse_type()?);
                if !self.eat(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::Gt, "`>` closing type arguments")?;
        }
        Ok(TypeExpr::Named { name, args, span: Span::new(start, nspan.hi) })
    }

    // ------------------------------------------------------------------
    // blocks & statements
    // ------------------------------------------------------------------
    pub fn parse_block(&mut self) -> Result<Block, ParseError> {
        let start = self.expect(&TokenKind::LBrace, "`{`")?.span.lo;
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut tail: Option<Box<Expr>> = None;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(ParseError { msg: "unterminated block".into(), span: self.peek().span });
            }
            if self.starts_statement_keyword() {
                stmts.push(self.parse_stmt()?);
                self.eat(&TokenKind::Semi);
                continue;
            }
            // 式（または代入文）
            let expr = self.parse_expr()?;
            // 代入演算子が続く場合は代入文
            let aop = match self.peek_kind() {
                TokenKind::Assign => Some(None::<BinOp>),
                TokenKind::PlusAssign => Some(Some(BinOp::Add)),
                TokenKind::MinusAssign => Some(Some(BinOp::Sub)),
                TokenKind::StarAssign => Some(Some(BinOp::Mul)),
                TokenKind::SlashAssign => Some(Some(BinOp::Div)),
                TokenKind::PercentAssign => Some(Some(BinOp::Mod)),
                _ => None,
            };
            if let Some(op) = aop {
                self.bump();
                let value = self.parse_expr()?;
                let span = expr.span.merge(value.span);
                stmts.push(Stmt::Assign { target: expr, op, value, span });
                self.eat(&TokenKind::Semi);
                continue;
            }
            if self.at(&TokenKind::RBrace) {
                tail = Some(Box::new(expr));
            } else {
                stmts.push(Stmt::Expr(expr));
                self.eat(&TokenKind::Semi);
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?.span.hi;
        Ok(Block { stmts, tail, span: Span::new(start, end) })
    }

    /// 文キーワードとして開始するトークンか（if/match/unsafe/{ は式でもあるが文扱い）
    fn starts_statement_keyword(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::KwLet | TokenKind::KwVar | TokenKind::KwReturn
                | TokenKind::KwWhile | TokenKind::KwFor | TokenKind::KwBreak
                | TokenKind::KwContinue | TokenKind::KwIf | TokenKind::KwMatch
                | TokenKind::KwUnsafe | TokenKind::LBrace
        )
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.peek_kind() {
            TokenKind::KwLet | TokenKind::KwVar => self.parse_let(),
            TokenKind::KwReturn => {
                let t = self.bump();
                let value = if self.at(&TokenKind::RBrace) || self.at(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Stmt::Return { value, span: t.span })
            }
            TokenKind::KwWhile => {
                let t = self.bump();
                let cond = self.parse_cond_expr()?;
                let body = self.parse_block()?;
                let span = Span::new(t.span.lo, body.span.hi);
                Ok(Stmt::While { cond, body, span })
            }
            TokenKind::KwFor => self.parse_for(),
            TokenKind::KwBreak => {
                let t = self.bump();
                Ok(Stmt::Break(t.span))
            }
            TokenKind::KwContinue => {
                let t = self.bump();
                Ok(Stmt::Continue(t.span))
            }
            // if / match / unsafe / block を文位置に書いた場合は値を捨てる式文
            TokenKind::KwIf | TokenKind::KwMatch | TokenKind::KwUnsafe | TokenKind::LBrace => {
                let e = self.parse_expr()?;
                Ok(Stmt::Expr(e))
            }
            _ => Err(self.err("expected statement")),
        }
    }

    fn parse_let(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump(); // let | var
        let mutable = matches!(kw.kind, TokenKind::KwVar);
        let (name, name_span) = self.ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let init = if self.eat(&TokenKind::Assign) {
            self.parse_expr()?
        } else {
            Expr { kind: ExprKind::Path(vec!["__uninit__".into()]), span: name_span }
        };
        let end = init.span.hi;
        Ok(Stmt::Let { name, name_span, ty, init, mutable, span: Span::new(kw.span.lo, end) })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        let kw = self.bump();
        // C スタイル: for i = 0; i < n; i += 1 { } は言語に代入文があるため
        // 「for pat in a..b」を基本形とし、in が来なければ C 風3節を許す
        let save = self.pos;
        if let Ok((var, _)) = self.ident() {
            if self.eat(&TokenKind::KwIn) {
                let start_e = self.parse_expr()?;
                self.expect(&TokenKind::DotDot, "`..` in range-for")?;
                let end_e = self.parse_expr()?;
                let body = self.parse_block()?;
                let span = Span::new(kw.span.lo, body.span.hi);
                return Ok(Stmt::ForRange { var, start: start_e, end: end_e, body, span });
            }
        }
        self.pos = save;
        // C-style: for init; cond; step { }
        let init: Option<Box<Stmt>> = if self.at(&TokenKind::Semi) {
            None
        } else if matches!(self.peek_kind(), TokenKind::KwLet | TokenKind::KwVar) {
            Some(Box::new(self.parse_let()?))
        } else {
            let target = self.parse_expr()?;
            if self.eat(&TokenKind::Assign) {
                let v = self.parse_expr()?;
                let sp = target.span.merge(v.span);
                Some(Box::new(Stmt::Assign { target, op: None, value: v, span: sp }))
            } else {
                Some(Box::new(Stmt::Expr(target)))
            }
        };
        self.expect(&TokenKind::Semi, "`;` after for-init")?;
        let cond = if self.at(&TokenKind::Semi) { None } else { Some(self.parse_expr()?) };
        self.expect(&TokenKind::Semi, "`;` after for-cond")?;
        let step: Option<Box<Stmt>> = if self.at(&TokenKind::LBrace) {
            None
        } else {
            let target = self.parse_expr()?;
            let op = match self.peek_kind() {
                TokenKind::Assign => Some(None::<BinOp>),
                TokenKind::PlusAssign => Some(Some(BinOp::Add)),
                TokenKind::MinusAssign => Some(Some(BinOp::Sub)),
                TokenKind::StarAssign => Some(Some(BinOp::Mul)),
                TokenKind::SlashAssign => Some(Some(BinOp::Div)),
                TokenKind::PercentAssign => Some(Some(BinOp::Mod)),
                _ => None,
            };
            if let Some(o) = op {
                self.bump();
                let v = self.parse_expr()?;
                let sp = target.span.merge(v.span);
                Some(Box::new(Stmt::Assign { target, op: o, value: v, span: sp }))
            } else {
                Some(Box::new(Stmt::Expr(target)))
            }
        };
        let body = self.parse_block()?;
        let span = Span::new(kw.span.lo, body.span.hi);
        Ok(Stmt::ForC { init, cond, step, body, span })
    }

    // ------------------------------------------------------------------
    // expressions
    // ------------------------------------------------------------------
    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_bin_expr(0)
    }

    /// if/match の条件・被検査体用。`x { .. }` を構造体リテラルにしない
    fn parse_cond_expr(&mut self) -> Result<Expr, ParseError> {
        self.no_struct_depth += 1;
        let e = self.parse_expr();
        self.no_struct_depth -= 1;
        e
    }

    fn struct_lit_allowed(&self) -> bool {
        self.no_struct_depth == 0
    }

    fn prec_of(k: &TokenKind) -> Option<u8> {
        Some(match k {
            TokenKind::OrOr => 1,
            TokenKind::AndAnd => 2,
            TokenKind::EqEq | TokenKind::NotEq => 3,
            TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge => 4,
            TokenKind::Plus | TokenKind::Minus => 5,
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 6,
            _ => return None,
        })
    }

    fn parse_bin_expr(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            // `as` キャスト（右結合・postfix 相当）
            if self.at(&TokenKind::KwAs) {
                self.bump();
                let ty = self.parse_type()?;
                let span = lhs.span.merge(ty.span());
                lhs = Expr { kind: ExprKind::Cast(Box::new(lhs), ty), span };
                continue;
            }
            // `?` 演算子（Result/Option 伝播、仕様 5.3）
            while self.at(&TokenKind::Question) {
                let q = self.bump();
                let span = lhs.span.merge(q.span);
                lhs = Expr { kind: ExprKind::Try(Box::new(lhs)), span };
            }
            let k = self.peek_kind();
            let prec = match Self::prec_of(&k) {
                Some(p) if p >= min_prec => p,
                _ => break,
            };
            self.bump();
            let op = match k {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::NotEq => BinOp::Neq,
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::Ge => BinOp::Ge,
                TokenKind::AndAnd => BinOp::And,
                TokenKind::OrOr => BinOp::Or,
                _ => unreachable!(),
            };
            let rhs = self.parse_bin_expr(prec + 1)?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr { kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let k = self.peek_kind();
        match k {
            TokenKind::Minus => {
                let t = self.bump();
                let e = self.parse_unary()?;
                let span = t.span.merge(e.span);
                Ok(Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(e)), span })
            }
            TokenKind::Bang => {
                let t = self.bump();
                let e = self.parse_unary()?;
                let span = t.span.merge(e.span);
                Ok(Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(e)), span })
            }
            // 単一 & はアドレス取得演算子 (&& は論理AND/ラムダ)
            TokenKind::Amp => {
                let t = self.bump();
                let e = self.parse_unary()?;
                let span = t.span.merge(e.span);
                Ok(Expr { kind: ExprKind::Unary(UnOp::AddrOf, Box::new(e)), span })
            }
            TokenKind::Star => {
                let t = self.bump();
                let e = self.parse_unary()?;
                let span = t.span.merge(e.span);
                Ok(Expr { kind: ExprKind::Unary(UnOp::Deref, Box::new(e)), span })
            }
            // ラムダ: `|| expr` は OrOr、`|a| ...` は Pipe
            TokenKind::PipePipe | TokenKind::Pipe => {
                let mut params = Vec::new();
                if self.eat(&TokenKind::PipePipe) {
                    // 引数なしラムダ
                } else {
                    self.expect(&TokenKind::Pipe, "`|`")?;
                    loop {
                        let (p, _) = self.ident()?;
                        params.push(p);
                        if !self.eat(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::Pipe, "`|` closing lambda params")?;
                }
                let body = Box::new(self.parse_expr()?);
                let span = body.span;
                Ok(Expr { kind: ExprKind::Lambda { params, body }, span })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek_kind() {
                TokenKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(&TokenKind::Comma) { break; }
                        }
                    }
                    let end = self.expect(&TokenKind::RParen, "`)` after call args")?.span.hi;
                    let span = Span::new(e.span.lo, end);
                    e = Expr {
                        kind: ExprKind::Call { callee: Box::new(e), args },
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    let end = self.expect(&TokenKind::RBracket, "`]` after index")?.span.hi;
                    let span = Span::new(e.span.lo, end);
                    e = Expr { kind: ExprKind::Index { base: Box::new(e), index: Box::new(idx) }, span };
                }
                TokenKind::Dot => {
                    self.bump();
                    let (field, _) = self.ident()?;
                    let span = Span::new(e.span.lo, e.span.hi + field.len() + 1);
                    e = Expr { kind: ExprKind::FieldAccess { base: Box::new(e), field }, span };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// f-string の `{expr}` 部分（独立したソース断片）を式として再パースする。
    fn parse_expr_from_src(&self, src: &str, span: Span) -> Result<Expr, ParseError> {
        let toks = ngs_lexer::lex(src).map_err(|e| ParseError { msg: e.msg, span })?;
        let mut p = Parser {
            src: src.to_string(),
            toks,
            pos: 0,
            jsx: self.jsx,
            no_struct_depth: 0,
            paren_depth: 0,
        };
        let e = p.parse_expr()?;
        if !p.at_eof() {
            return Err(ParseError {
                msg: "unexpected token in string interpolation".into(),
                span,
            });
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let t = self.peek();
        match t.kind {
            TokenKind::IntLit(v) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Int(v), span: t.span })
            }
            TokenKind::FloatLit(v) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Float(v), span: t.span })
            }
            TokenKind::KwTrue => {
                self.bump();
                Ok(Expr { kind: ExprKind::Bool(true), span: t.span })
            }
            TokenKind::KwFalse => {
                self.bump();
                Ok(Expr { kind: ExprKind::Bool(false), span: t.span })
            }
            TokenKind::KwNull => {
                self.bump();
                Ok(Expr { kind: ExprKind::Null, span: t.span })
            }
            TokenKind::StrLit(s) => {
                self.bump();
                Ok(Expr { kind: ExprKind::Str(s), span: t.span })
            }
            TokenKind::FStr(segs) => {
                self.bump();
                let mut parts = Vec::with_capacity(segs.len());
                for seg in segs {
                    match seg {
                        ngs_ast::FStrSeg::Text(s) => parts.push(ngs_ast::FStringPart::Text(s)),
                        ngs_ast::FStrSeg::Expr(sp) => {
                            let src = self.src.get(sp.lo..sp.hi).unwrap_or_default().to_string();
                            let e = self.parse_expr_from_src(&src, sp)?;
                            parts.push(ngs_ast::FStringPart::Expr(Box::new(e)));
                        }
                    }
                }
                Ok(Expr { kind: ExprKind::FStr(parts), span: t.span })
            }
            TokenKind::LParen => {
                const MAX_PAREN_DEPTH: u32 = 64;
                if self.paren_depth >= MAX_PAREN_DEPTH {
                    return Err(ParseError {
                        msg: format!(
                            "parenthesized expression nesting exceeds {MAX_PAREN_DEPTH} levels; split the expression"
                        ),
                        span: t.span,
                    });
                }
                self.paren_depth += 1;
                self.bump();
                let r = self.parse_expr().and_then(|e| {
                    self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(e)
                });
                self.paren_depth -= 1;
                r
            }
            TokenKind::LBracket => {
                self.bump();
                let mut elems = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    loop {
                        elems.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) { break; }
                    }
                }
                let end = self.expect(&TokenKind::RBracket, "`]` after array literal")?.span.hi;
                Ok(Expr { kind: ExprKind::ArrayLit(elems), span: Span::new(t.span.lo, end) })
            }
            TokenKind::KwIf => self.parse_if(),
            TokenKind::KwMatch => self.parse_match(),
            TokenKind::LBrace => {
                let b = self.parse_block()?;
                let span = b.span;
                Ok(Expr { kind: ExprKind::BlockExpr(b), span })
            }
            TokenKind::KwUnsafe => {
                self.bump();
                let b = self.parse_block()?;
                let span = b.span;
                Ok(Expr { kind: ExprKind::UnsafeBlock(b), span })
            }
            TokenKind::Ident(_) => {
                // struct リテラル: Ident { field: expr, ... } — `{` が直後にある場合のみ
                if let TokenKind::Ident(ref n) = t.kind {
                    if self.struct_lit_allowed()
                        && matches!(self.peek2_kind(), TokenKind::LBrace)
                        && !self.jsx_component_like(n)
                    {
                        let name = n.clone();
                        self.bump();
                        self.bump(); // {
                        let mut fields = Vec::new();
                        while !self.at(&TokenKind::RBrace) {
                            let (fname, _) = self.ident()?;
                            self.expect(&TokenKind::Colon, "`:` after field name")?;
                            let val = self.parse_expr()?;
                            fields.push((fname, val));
                            if !self.eat(&TokenKind::Comma) { break; }
                        }
                        let end = self.expect(&TokenKind::RBrace, "`}` closing struct literal")?.span.hi;
                        return Ok(Expr {
                            kind: ExprKind::StructLit { name, fields },
                            span: Span::new(t.span.lo, end),
                        });
                    }
                }
                // パス: a / a.b / a.b.c
                let (first, fspan) = self.ident()?;
                let mut path = vec![first];
                while self.at(&TokenKind::Dot) {
                    self.bump();
                    let (seg, _) = self.ident()?;
                    path.push(seg);
                }
                let end = self.toks[self.pos - 1].span.hi;
                Ok(Expr { kind: ExprKind::Path(path), span: Span::new(fspan.lo, end) })
            }
            TokenKind::Lt if self.jsx => self.parse_jsx_element(),
            _ => Err(self.err("expected expression")),
        }
    }

    /// 大文字開始の識別子（JSXコンポーネント候補）か
    fn jsx_component_like(&self, name: &str) -> bool {
        self.jsx && name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
            && false // structリテラル優先のため無効化。コンポーネント呼び出しは <Comp /> のみ
    }

    // ------------------------------------------------------------------
    // if / match
    // ------------------------------------------------------------------
    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::KwIf, "`if`")?;
        let cond = self.parse_cond_expr()?;
        let then_body = self.parse_block()?;
        let else_body = if self.eat(&TokenKind::KwElse) {
            if self.at(&TokenKind::KwIf) {
                Some(Box::new(self.parse_if()?))
            } else {
                let b = self.parse_block()?;
                let span = b.span;
                Some(Box::new(Expr { kind: ExprKind::BlockExpr(b), span }))
            }
        } else {
            None
        };
        let end = else_body.as_ref().map(|e| e.span.hi).unwrap_or(then_body.span.hi);
        Ok(Expr {
            kind: ExprKind::If { cond: Box::new(cond), then_body, else_body },
            span: Span::new(kw.span.lo, end),
        })
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        let kw = self.expect(&TokenKind::KwMatch, "`match`")?;
        let scrutinee = self.parse_cond_expr()?;
        self.expect(&TokenKind::LBrace, "`{` starting match arms")?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::FatArrow, "`=>` after match pattern")?;
            let body = self.parse_match_arm_body()?;
            arms.push(MatchArm { pattern, body });
            self.eat(&TokenKind::Comma);
        }
        let end = self.expect(&TokenKind::RBrace, "`}` closing match")?.span.hi;
        Ok(Expr {
            kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms },
            span: Span::new(kw.span.lo, end),
        })
    }

    fn parse_match_arm_body(&mut self) -> Result<Expr, ParseError> {
        if self.at(&TokenKind::LBrace) {
            let b = self.parse_block()?;
            let span = b.span;
            Ok(Expr { kind: ExprKind::BlockExpr(b), span })
        } else {
            self.parse_expr()
        }
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let t = self.peek();
        match t.kind {
            TokenKind::IntLit(v) => {
                self.bump();
                Ok(Pattern { kind: PatternKind::Int(v as i64), span: t.span })
            }
            TokenKind::FloatLit(_) => Err(self.err("float patterns are not supported")),
            TokenKind::KwTrue => {
                self.bump();
                Ok(Pattern { kind: PatternKind::Bool(true), span: t.span })
            }
            TokenKind::KwFalse => {
                self.bump();
                Ok(Pattern { kind: PatternKind::Bool(false), span: t.span })
            }
            TokenKind::StrLit(s) => {
                self.bump();
                Ok(Pattern { kind: PatternKind::Str(s), span: t.span })
            }
            TokenKind::Ident(name) => {
                if name == "_" {
                    self.bump();
                    return Ok(Pattern { kind: PatternKind::Wildcard, span: t.span });
                }
                self.bump();
                // Variant(payloads...) 形式
                let mut bindings = Vec::new();
                if self.eat(&TokenKind::LParen) {
                    loop {
                        let (b, _) = self.ident()?;
                        bindings.push(b);
                        if !self.eat(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen, "`)` after pattern bindings")?;
                }
                Ok(Pattern {
                    kind: PatternKind::Variant { enum_name: None, variant: name, bindings },
                    span: t.span,
                })
            }
            _ => Err(self.err("expected match pattern")),
        }
    }

    // ------------------------------------------------------------------
    // JSX (`.ngsx`) → createElement 呼び出しへの糖衣展開
    // ------------------------------------------------------------------
    fn parse_jsx_element(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(&TokenKind::Lt, "`<`")?.span.lo;
        // fragment <>...</>
        if self.eat(&TokenKind::Gt) {
            let children = self.parse_jsx_children()?;
            let close = self.expect(&TokenKind::Lt, "`<` of closing tag")?;
            let _ = close;
            self.expect(&TokenKind::Gt, "`>` closing fragment")?;
            let end = self.toks[self.pos - 1].span.hi;
            let props = Expr {
                kind: ExprKind::JsxProps(Vec::new()),
                span: Span::new(start, end),
            };
            let mut args = vec![
                Expr { kind: ExprKind::Str("#fragment".into()), span: Span::new(start, end) },
                props,
            ];
            args.extend(children);
            return Ok(Expr {
                kind: ExprKind::Call {
                    callee: Box::new(Expr {
                        kind: ExprKind::Path(vec!["createElement".into()]),
                        span: Span::new(start, end),
                    }),
                    args,
                },
                span: Span::new(start, end),
            });
        }
        let (tag, tag_span) = self.ident()?;
        let is_component = tag.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);

        // attributes
        let mut attrs: Vec<(String, Expr)> = Vec::new();
        loop {
            match self.peek_kind() {
                TokenKind::Ident(_) => {
                    let (aname, aspan) = self.ident()?;
                    self.expect(&TokenKind::Assign, "`=` after attribute name")?;
                    let val = match self.peek_kind() {
                        TokenKind::StrLit(s) => {
                            let s = s.clone();
                            let t = self.bump();
                            Expr { kind: ExprKind::Str(s), span: t.span }
                        }
                        TokenKind::LBrace => {
                            self.bump();
                            let e = self.parse_expr()?;
                            self.expect(&TokenKind::RBrace, "`}` closing attribute expression")?
                                .span
                                .hi;
                            e
                        }
                        _ => {
                            return Err(self.err(
                                "attribute value must be a string literal or {expression}",
                            ))
                        }
                    };
                    attrs.push((aname, val));
                    let _ = aspan;
                }
                TokenKind::Slash => {
                    // 自己閉じタグ
                    self.bump();
                    self.expect(&TokenKind::Gt, "`>` after `/`")?;
                    let end = self.toks[self.pos - 1].span.hi;
                    return Ok(self.build_create_element_call(&tag, is_component, attrs, Vec::new(), start, end));
                }
                TokenKind::Gt => {
                    self.bump();
                    let children = self.parse_jsx_children()?;
                    // </tag>
                    self.expect(&TokenKind::Lt, "`<` of closing tag")?;
                    self.expect(&TokenKind::Slash, "`/` of closing tag")?;
                    let (ctag, cspan) = self.ident()?;
                    if ctag != tag {
                        return Err(ParseError {
                            msg: format!("mismatched closing tag `</{ctag}>`, expected `</{tag}>`"),
                            span: cspan,
                        });
                    }
                    let end = self.expect(&TokenKind::Gt, "`>` closing tag")?.span.hi;
                    return Ok(self.build_create_element_call(&tag, is_component, attrs, children, start, end));
                }
                _ => return Err(self.err("expected attribute, `>` or `/>` in JSX element")),
            }
        }
    }

    fn build_create_element_call(
        &self,
        tag: &str,
        is_component: bool,
        attrs: Vec<(String, Expr)>,
        children: Vec<Expr>,
        lo: usize,
        hi: usize,
    ) -> Expr {
        let span = Span::new(lo, hi);
        let tag_arg = if is_component {
            Expr { kind: ExprKind::Path(vec![tag.to_string()]), span }
        } else {
            Expr { kind: ExprKind::Str(tag.to_string()), span }
        };
        let props = Expr { kind: ExprKind::JsxProps(attrs), span };
        let mut args = vec![tag_arg, props];
        args.extend(children);
        Expr {
            kind: ExprKind::Call {
                callee: Box::new(Expr {
                    kind: ExprKind::Path(vec!["createElement".into()]),
                    span,
                }),
                args,
            },
            span,
        }
    }

    fn parse_jsx_children(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut children: Vec<Expr> = Vec::new();
        // テキストセグメントはソース原テキストから復元する
        let mut seg_start = self.peek().span.lo;
        loop {
            let t = self.peek();
            match t.kind {
                TokenKind::Lt => {
                    self.push_text_segment(&mut children, seg_start, t.span.lo);
                    if self.peek2_kind() == TokenKind::Slash {
                        return Ok(children); // 閉じタグは呼び出し側が処理
                    }
                    let el = self.parse_jsx_element()?;
                    children.push(el);
                    seg_start = self.peek().span.lo;
                }
                TokenKind::LBrace => {
                    self.push_text_segment(&mut children, seg_start, t.span.lo);
                    self.bump();
                    let e = self.parse_expr()?;
                    self.expect(&TokenKind::RBrace, "`}` closing interpolation")?;
                    children.push(e);
                    seg_start = self.peek().span.lo;
                }
                TokenKind::Eof => {
                    return Err(ParseError { msg: "unterminated JSX element".into(), span: t.span })
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// ソース [lo, hi) を生テキストとして取り出し、空白のみでなければ Str 子ノードにする
    fn push_text_segment(&self, children: &mut Vec<Expr>, lo: usize, hi: usize) {
        if hi <= lo || hi > self.src.len() || lo > self.src.len() {
            return;
        }
        let raw = &self.src[lo..hi];
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            children.push(Expr {
                kind: ExprKind::Str(trimmed.to_string()),
                span: Span::new(lo, hi),
            });
        }
    }
}

