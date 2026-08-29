//! NagiScript AST + shared token vocabulary.
//!
//! `.ngs` / `.ngsx` 両方のフロントエンドで共有されるデータ型。
//! Span はバイトオフセット (lo, hi) のみを保持し、行/列は必要時に
//! ソースから計算する（lexer/driver で line_index を持つ方式）。

pub type TypeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub lo: usize,
    pub hi: usize,
}

impl Span {
    pub fn new(lo: usize, hi: usize) -> Self {
        Span { lo, hi }
    }
    pub fn merge(self, other: Span) -> Span {
        Span {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// f-string のセグメント。`f"a{x}b"` は Text("a") / Expr(span) / Text("b")。
/// Expr の span は `{...}` の中身（波括弧を除いたソース範囲）を指し、
/// parser がその範囲を再パースして式へ変換する。
#[derive(Debug, Clone, PartialEq)]
pub enum FStrSeg {
    Text(String),
    Expr(Span),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    IntLit(u64),
    FloatLit(f64),
    StrLit(String),
    /// `f"..."` 文字列補間（print/println 内で print-time 展開される）
    FStr(Vec<FStrSeg>),

    // keywords (spec 4: fn val var if else for while match struct enum unsafe extern export return)
    KwFn,
    KwLet, // internally "let" but source keyword is "val"
    KwVar,
    KwIf,
    KwElse,
    KwFor,
    KwWhile,
    KwMatch,
    KwStruct,
    KwEnum,
    KwUnsafe,
    KwExtern,
    KwExport,
    KwReturn,
    // additions beyond the minimal spec keyword set (documented in docs/NOTES.md)
    KwImpl,
    KwIn,
    KwAs,
    KwBreak,
    KwContinue,
    KwTrue,
    KwFalse,

    // operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    Amp,
    OrOr,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    Arrow,     // ->
    FatArrow,  // =>
    Question,  // ?
    At,        // @

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDot,

    Pipe,      // |  (lambda params)
    PipePipe,  // || (or-op / empty lambda)

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct File {
    pub items: Vec<Item>,
    pub jsx: bool,
    pub path: String,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Impl(ImplBlock),
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub name_span: Span,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    /// None => extern 宣言（本体なし）
    pub body: Option<Block>,
    /// extern "C" fn ...; の宣言
    pub extern_abi: Option<String>,
    /// export "C" fn ...
    pub export_abi: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub name_span: Span,
    pub type_params: Vec<String>,
    pub fields: Vec<FieldDef>,
    pub repr_c: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub name_span: Span,
    pub type_params: Vec<String>,
    pub variants: Vec<VariantDef>,
    pub repr_c: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub payload_types: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub type_name: String,
    pub type_params: Vec<String>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Types (syntactic)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// i32, List<i32>, Result<T, E>, Point
    Named { name: String, args: Vec<TypeExpr>, span: Span },
    /// *T
    Ptr { elem: Box<TypeExpr>, span: Span },
    /// [T; N]
    Array { elem: Box<TypeExpr>, len: u64, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. } => *span,
            TypeExpr::Ptr { span, .. } => *span,
            TypeExpr::Array { span, .. } => *span,
        }
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

/// 二項演算子（代入複合演算にも流用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    AddrOf,
    Deref,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        name_span: Span,
        ty: Option<TypeExpr>,
        init: Expr,
        mutable: bool,
        span: Span,
    },
    Assign {
        target: Expr,
        op: Option<BinOp>, // None => =
        value: Expr,
        span: Span,
    },
    Expr(Expr),
    Return {
        value: Option<Expr>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    ForRange {
        var: String,
        start: Expr,
        end: Expr,
        body: Block,
        span: Span,
    },
    ForC {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Box<Stmt>>,
        body: Block,
        span: Span,
    },
    Break(Span),
    Continue(Span),
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(u64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// `f"..."` 文字列補間。セグメントは Text または埋め込み式。
    FStr(Vec<FStringPart>),
    /// 識別子または a.b.c 形式のパス。semaが変数/フィールド/関連関数/
    /// enumバリアントに解決する。
    Path(Vec<String>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, TypeExpr),
    Call { callee: Box<Expr>, args: Vec<Expr> },
    Index { base: Box<Expr>, index: Box<Expr> },
    FieldAccess { base: Box<Expr>, field: String },
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    /// Circle(r) / Shape.Circle(r) のようなenumバリアント生成
    VariantCtor {
        enum_name: Option<String>,
        variant: String,
        payloads: Vec<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    If {
        cond: Box<Expr>,
        then_body: Block,
        else_body: Option<Box<Expr>>, // BlockExpr または If
    },
    BlockExpr(Block),
    UnsafeBlock(Block),
    ArrayLit(Vec<Expr>),
    /// `expr?` — Result/Option のエラー伝播 (spec 5.3)
    Try(Box<Expr>),
    /// `|| expr` / `|a, b| expr` — Stage 8 ではパースのみ
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },
    /// JSX属性集合。parser が `<div class="x">{e}</div>` を
    /// createElement("div", JsxProps([...]), ...) 呼び出しへ糖衣展開した結果の一部。
    JsxProps(Vec<(String, Expr)>),
}

#[derive(Debug, Clone)]
pub enum FStringPart {
    Text(String),
    /// 補間式（`{expr}` の中身）。parser が FStr トークンの Expr セグメントを
    /// 再パースした結果。
    Expr(Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatternKind {
    Wildcard,
    Int(i64),
    Bool(bool),
    Str(String),
    /// Circle(r) / Shape.Circle(r)。bindings は payload 束縛名
    Variant {
        enum_name: Option<String>,
        variant: String,
        bindings: Vec<String>,
    },
}
