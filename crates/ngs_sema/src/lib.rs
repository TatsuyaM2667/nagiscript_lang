//! NagiScript 意味解析 (Sema)。
//! - 名前解決・型検査
//! - Zig/Odin 方式のコンパイル時ダックタイピング的ジェネリクス（モノモーフィゼーション）
//! - safe/unsafe 境界チェック（生ポインタの参照外しは unsafe 内のみ）
//! - match 網羅性チェック
//! - export 関数シグネチャの収集（Wasm/.d.ts 生成用）
//!
//! 出力: TypedProgram（NGS-IR への入力）

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ngs_ast::*;
use TExprKind::*;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Void,
    Bool,
    Str,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Isize,
    F32,
    F64,
    Ptr(Rc<Ty>),
    Array(Rc<Ty>, u64),
    /// 構造体インスタンス（def_id, 型引数）
    Struct(usize, Vec<Ty>),
    /// タグ付きenumインスタンス
    Enum(usize, Vec<Ty>),
    /// テンプレート中の型パラメータ
    Generic(String),
    /// 組み込み参照カウントポインタ
    RcT(Rc<Ty>),
    /// JSX属性集合（opaque）
    Props,
}

impl Ty {
    pub fn is_int(&self) -> bool {
        matches!(
            self,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
                | Ty::Usize | Ty::Isize
        )
    }
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }
    pub fn is_ptr(&self) -> bool {
        matches!(self, Ty::Ptr(_) | Ty::RcT(_))
    }
    pub fn is_aggregate(&self) -> bool {
        matches!(self, Ty::Array(..) | Ty::Struct(..))
    }
    /// 整数型のビット幅（それ以外は None）。縮小変換判定に使う。
    pub fn int_bits(&self) -> Option<u32> {
        match self {
            Ty::I8 | Ty::U8 => Some(8),
            Ty::I16 | Ty::U16 => Some(16),
            Ty::I32 | Ty::U32 => Some(32),
            Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => Some(64),
            _ => None,
        }
    }
    /// 型名（エラー表示・マングリング用）
    pub fn display(&self) -> String {
        match self {
            Ty::Void => "void".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "string".into(),
            Ty::I8 => "i8".into(),
            Ty::I16 => "i16".into(),
            Ty::I32 => "i32".into(),
            Ty::I64 => "i64".into(),
            Ty::U8 => "u8".into(),
            Ty::U16 => "u16".into(),
            Ty::U32 => "u32".into(),
            Ty::U64 => "u64".into(),
            Ty::Usize => "usize".into(),
            Ty::Isize => "isize".into(),
            Ty::F32 => "f32".into(),
            Ty::F64 => "f64".into(),
            Ty::Ptr(t) => format!("*{}", t.display()),
            Ty::Array(t, n) => format!("[{}; {}]", t.display(), n),
            Ty::Struct(id, subs) => named_display(self.struct_name(*id), subs),
            Ty::Enum(id, subs) => named_display(self.enum_name(*id), subs),
            Ty::Generic(n) => n.clone(),
            Ty::RcT(t) => format!("Rc<{}>", t.display()),
            Ty::Props => "Props".into(),
        }
    }
    fn struct_name(&self, id: usize) -> String {
        STRUCT_BUILTIN_NAMES.get(id).map(|x| x.to_string()).unwrap_or(format!("<struct#{id}>"))
    }
    fn enum_name(&self, id: usize) -> String {
        ENUM_BUILTIN_NAMES.get(id).map(|x| x.to_string()).unwrap_or(format!("<enum#{id}>"))
    }
}

fn named_display(name: String, subs: &[Ty]) -> String {
    if subs.is_empty() {
        name
    } else {
        let inner: Vec<String> = subs.iter().map(|t| t.display()).collect();
        format!("{}<{}>", name, inner.join(", "))
    }
}

/// ビルトイン型のID予約
pub const BUILTIN_LIST: usize = 0;
pub const BUILTIN_RESULT: usize = 1;
pub const BUILTIN_OPTION: usize = 2;
const STRUCT_BUILTIN_NAMES: [&str; 3] = ["List", "__unused__", "__unused__"];
const ENUM_BUILTIN_NAMES: [&str; 3] = ["__unused__", "Result", "Option"];

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<(String, TypeExpr)>,
    pub repr_c: bool,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<(String, Vec<TypeExpr>)>,
    pub repr_c: bool,
}

#[derive(Debug, Clone)]
pub struct FnTemplate {
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<(String, TypeExpr)>,
    pub ret: Option<TypeExpr>,
    pub body: Option<ngs_ast::Block>,
    pub extern_abi: Option<String>,
    pub export_abi: bool,
    /// impl ブロックの所有型名
    pub owner_type: Option<String>,
    /// メソッドか（第1引数がself）
    pub is_method: bool,
}

// ---------------------------------------------------------------------------
// Typed output (HIR) — NGS-IR lowering の入力
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MonoStruct {
    pub def_id: usize,
    pub substs: Vec<Ty>,
    pub mangled: String,
    pub fields: Vec<(String, Ty)>,
    pub is_list: bool,
}

#[derive(Debug, Clone)]
pub struct MonoEnum {
    pub def_id: usize,
    pub substs: Vec<Ty>,
    pub mangled: String,
    pub variants: Vec<(String, Vec<Ty>)>,
}

#[derive(Debug, Clone)]
pub struct MonoFn {
    pub mangled: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub body: Option<TBlock>,
    pub extern_abi: Option<String>,
    pub export_abi: bool,
    pub is_user_main: bool,
}

#[derive(Debug, Clone)]
pub struct ExportSig {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
}

#[derive(Debug, Clone, Default)]
pub struct TypedProgram {
    pub structs: Vec<MonoStruct>,
    pub enums: Vec<MonoEnum>,
    pub funcs: Vec<MonoFn>,
    pub exports: Vec<ExportSig>,
    pub has_main: bool,
}

#[derive(Debug, Clone)]
pub struct TBlock {
    pub stmts: Vec<TStmt>,
    pub tail: Option<Box<TExpr>>,
}

#[derive(Debug, Clone)]
pub enum TStmt {
    Let(String, Ty, TExpr, bool),
    Assign(TExpr, Option<BinOp>, TExpr),
    Expr(TExpr),
    Return(Option<TExpr>),
    While(TExpr, TBlock),
    ForRange(String, Ty, TExpr, TExpr, TBlock),
    ForC(Option<Box<TStmt>>, Option<TExpr>, Option<Box<TStmt>>, TBlock),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub struct TExpr {
    pub kind: TExprKind,
    pub ty: Ty,
    pub span: Span,
    /// 安全な文脈で生成された整数演算か（オーバーフロー検出を下流で挿入）
    pub checked: bool,
}

#[derive(Debug, Clone)]
pub enum Callee {
    /// ユーザー定義（モノモーフ化済み）関数を直接呼ぶ
    Direct(String),
    Extern(String),
    Intrinsic(Intrinsic),
}

#[derive(Debug, Clone)]
pub enum Intrinsic {
    Print { newline: bool, value_ty: Ty },
    /// f-string を print/println で print-time 展開する。
    /// parts の Text はそのまま出力、Expr は型に応じた print 呼び出しへ。
    PrintFStr { newline: bool, parts: Vec<TFStringPart> },
    Len,
    Panic,
    Abort,
    SizeOfStr,
    ListNew,
    ListPush,
    ListGet,
    ListSet,
    ListLen,
    RcNew,
    RcGet,
    StrEq,
    /// JSX props / 値のボックス化（codegenでタグ付き値へ）
    PropsNew,
    PropsSet { value_ty: Ty },
    BoxAny { value_ty: Ty },
}

#[derive(Debug, Clone)]
pub enum TExprKind {
    Int(u64),
    Float(f64),
    Bool(bool),
    Null,
    Str(String),
    Local(String),
    Unary(UnOp, Box<TExpr>),
    Binary(BinOp, Box<TExpr>, Box<TExpr>),
    Cast(Box<TExpr>),
    Call(Callee, Vec<TExpr>),
    Index { base: Box<TExpr>, index: Box<TExpr> },
    Field { base: Box<TExpr>, index: usize },
    Deref(Box<TExpr>),
    AddrOf(Box<TExpr>),
    StructLit { mangled: String, fields: Vec<(usize, TExpr)> },
    VariantCtor { mangled: String, variant: usize, payloads: Vec<TExpr> },
    Match { scrutinee: Box<TExpr>, arms: Vec<TArm> },
    If { cond: Box<TExpr>, then_body: TBlock, else_body: Option<Box<TExpr>> },
    Block(TBlock),
    Try(Box<TExpr>),
    ArrayLit(Vec<TExpr>),
    Props(Vec<(String, TExpr)>),
    UninitPlaceholder,
}

#[derive(Debug, Clone)]
pub struct TArm {
    pub pattern: TPattern,
    pub body: TExpr,
}

#[derive(Debug, Clone)]
pub enum TPattern {
    Wildcard,
    Int(i64),
    Bool(bool),
    Str(String),
    Variant { mangled: String, variant: usize, bindings: Vec<(String, Ty)> },
}

/// f-string の型検査済みセグメント（print-time 展開用）
#[derive(Debug, Clone)]
pub enum TFStringPart {
    Text(String),
    Expr(TExpr),
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SemaError {
    pub msg: String,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Checker
// ---------------------------------------------------------------------------

struct LocalVar {
    ty: Ty,
    mutable: bool,
}

pub struct Checker {
    structs: Vec<StructDef>,
    enums: Vec<EnumDef>,
    fns: HashMap<String, FnTemplate>,
    /// enumバリアント名 → 所有enum def のリスト（曖昧性解決用）
    variant_owners: HashMap<String, Vec<usize>>,
    out: TypedProgram,
    done_fns: HashSet<String>,
    pending: Vec<(String, Vec<Ty>)>, // (内部キー, substs)
    /// 現在チェック中の関数の戻り値型
    cur_ret: Ty,
    unsafe_depth: u32,
    /// 式検査の再帰深さ（addendum 3.4: 病的ネストへの静的上限）
    expr_depth: u32,
    scopes: Vec<HashMap<String, LocalVar>>,
    loop_depth: u32,
    errors: Vec<SemaError>,
}

/// 内部テンプレートキー: トップレベルは名前、メソッドは "Type::method"
fn method_key(owner: &str, name: &str) -> String {
    format!("{}::{}", owner, name)
}

/// マングリング: name__i32__F32 のように型引数を連結
fn mangle(base: &str, substs: &[Ty]) -> String {
    if substs.is_empty() {
        sanitize(base)
    } else {
        let mut s = sanitize(base);
        for t in substs {
            s.push_str("__");
            s.push_str(&sanitize(&t.display()));
        }
        s
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '<' | '>' | ',' | ' ' | '*' | '[' | ']' | ';' => '_',
            _ => c,
        })
        .collect()
}

pub fn check(file: &File) -> Result<TypedProgram, Vec<SemaError>> {
    let mut c = Checker {
        structs: vec![
            StructDef { name: "List".into(), type_params: vec!["T".into()], fields: vec![], repr_c: false },
            StructDef { name: "__unused_struct".into(), type_params: vec![], fields: vec![], repr_c: false },
            StructDef { name: "__unused_struct2".into(), type_params: vec![], fields: vec![], repr_c: false },
        ],
        enums: vec![
            EnumDef { name: "__unused_enum".into(), type_params: vec![], variants: vec![], repr_c: false },
            EnumDef {
                name: "Result".into(),
                type_params: vec!["T".into(), "E".into()],
                variants: vec![
                    ("Ok".into(), vec![te_named("T")]),
                    ("Err".into(), vec![te_named("E")]),
                ],
                repr_c: false,
            },
            EnumDef {
                name: "Option".into(),
                type_params: vec!["T".into()],
                variants: vec![
                    ("Some".into(), vec![te_named("T")]),
                    ("None".into(), vec![]),
                ],
                repr_c: false,
            },
        ],
        fns: HashMap::new(),
        variant_owners: {
            let mut vo: HashMap<String, Vec<usize>> = HashMap::new();
            vo.insert("Ok".into(), vec![BUILTIN_RESULT]);
            vo.insert("Err".into(), vec![BUILTIN_RESULT]);
            vo.insert("Some".into(), vec![BUILTIN_OPTION]);
            vo.insert("None".into(), vec![BUILTIN_OPTION]);
            vo
        },
        out: TypedProgram::default(),
        done_fns: HashSet::new(),
        pending: Vec::new(),
        cur_ret: Ty::Void,
        unsafe_depth: 0,
        expr_depth: 0,
        scopes: vec![HashMap::new()],
        loop_depth: 0,
        errors: Vec::new(),
    };
    c.collect_items(file);
    c.seed_entry_points(file);
    c.run_worklist();
    c.finalize_exports(file);
    if c.errors.is_empty() {
        Ok(c.out)
    } else {
        Err(c.errors)
    }
}

fn te_named(name: &str) -> TypeExpr {
    TypeExpr::Named { name: name.into(), args: vec![], span: Span::new(0, 0) }
}

impl Checker {
    // ------------------------------------------------------------------
    // 収集フェーズ
    // ------------------------------------------------------------------
    fn collect_items(&mut self, file: &File) {
        for item in &file.items {
            match item {
                Item::Struct(sd) => {
                    self.structs.push(StructDef {
                        name: sd.name.clone(),
                        type_params: sd.type_params.clone(),
                        fields: sd.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
                        repr_c: sd.repr_c,
                    });
                }
                Item::Enum(ed) => {
                    let id = self.enums.len();
                    for v in &ed.variants {
                        self.variant_owners.entry(v.name.clone()).or_default().push(id);
                    }
                    self.enums.push(EnumDef {
                        name: ed.name.clone(),
                        type_params: ed.type_params.clone(),
                        variants: ed.variants.iter().map(|v| (v.name.clone(), v.payload_types.clone())).collect(),
                        repr_c: ed.repr_c,
                    });
                }
                Item::Fn(fd) => {
                    let key = fd.name.clone();
                    if self.fns.contains_key(&key) {
                        self.err(format!("duplicate function `{}`", fd.name), fd.name_span);
                        continue;
                    }
                    self.fns.insert(key, FnTemplate {
                        name: fd.name.clone(),
                        type_params: fd.type_params.clone(),
                        params: fd.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
                        ret: fd.ret.clone(),
                        body: fd.body.clone(),
                        extern_abi: fd.extern_abi.clone(),
                        export_abi: fd.export_abi.is_some(),
                        owner_type: None,
                        is_method: false,
                    });
                }
                Item::Impl(im) => {
                    for m in &im.methods {
                        let key = method_key(&im.type_name, &m.name);
                        if self.fns.contains_key(&key) {
                            self.err(
                                format!("duplicate method `{}.{}`", im.type_name, m.name),
                                m.name_span,
                            );
                            continue;
                        }
                        let is_method = m.params.first().map(|p| p.name == "self").unwrap_or(false);
                        self.fns.insert(key, FnTemplate {
                            name: m.name.clone(),
                            type_params: {
                                let mut tp = im.type_params.clone();
                                for x in &m.type_params {
                                    tp.push(x.clone());
                                }
                                tp
                            },
                            params: m.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect(),
                            ret: m.ret.clone(),
                            body: m.body.clone(),
                            extern_abi: m.extern_abi.clone(),
                            export_abi: m.export_abi.is_some(),
                            owner_type: Some(im.type_name.clone()),
                            is_method,
                        });
                    }
                }
            }
        }
    }

    /// エントリポイント（非ジェネリック関数）をワークリストに投入
    fn seed_entry_points(&mut self, file: &File) {
        let mut mains = 0;
        for item in &file.items {
            if let Item::Fn(fd) = item {
                if fd.body.is_some() && fd.type_params.is_empty() {
                    if fd.name == "main" {
                        mains += 1;
                        self.out.has_main = true;
                    }
                    self.pending.push((fd.name.clone(), vec![]));
                }
            }
        }
        if mains > 1 {
            self.err("multiple `main` functions".to_string(), Span::new(0, 0));
        }
        // 具象型のimplメソッドも投入（使われなくてもexport等のため）
        let keys: Vec<String> = self.fns.keys().cloned().collect();
        for key in keys {
            if !key.contains("::") {
                continue;
            }
            let tpl = &self.fns[&key];
            let owner = tpl.owner_type.clone().unwrap_or_default();
            let owner_is_generic_struct = self
                .lookup_struct(&owner)
                .map(|(_, d, _)| !d.type_params.is_empty())
                .unwrap_or(false);
            let owner_is_generic_enum = self
                .lookup_enum(&owner)
                .map(|(_, d, _)| !d.type_params.is_empty())
                .unwrap_or(false);
            if !(owner_is_generic_struct || owner_is_generic_enum) && tpl.body.is_some() {
                self.pending.push((key.clone(), vec![]));
            }
        }
    }

    fn run_worklist(&mut self) {
        while let Some((key, substs)) = self.pending.pop() {
            let mangled_base;
            {
                let Some(tpl) = self.fns.get(&key) else { continue };
                mangled_base = mangle(&display_key(&key), &substs);
            }
            if self.done_fns.contains(&mangled_base) {
                continue;
            }
            self.done_fns.insert(mangled_base.clone());
            if let Err(e) = self.instantiate_and_check(key, substs, mangled_base) {
                self.errors.push(e);
            }
        }
    }

    fn instantiate_and_check(&mut self, key: String, substs: Vec<Ty>, mangled: String) -> Result<(), SemaError> {
        let tpl = self.fns.get(&key).cloned().ok_or_else(|| SemaError {
            msg: format!("internal: missing template `{key}`"),
            span: Span::new(0, 0),
        })?;

        // subst マップ構築
        let mut subst_map: HashMap<String, Ty> = HashMap::new();
        for (i, p) in tpl.type_params.iter().enumerate() {
            let t = substs.get(i).cloned().unwrap_or(Ty::Void);
            subst_map.insert(p.clone(), t);
        }

        // シグネチャ変換
        let mut params_typed = Vec::new();
        for (pname, pty) in &tpl.params {
            params_typed.push((pname.clone(), self.conv_ty(pty, &subst_map, Span::new(0, 0))?));
        }
        let ret = match &tpl.ret {
            Some(rt) => self.conv_ty(rt, &subst_map, Span::new(0, 0))?,
            None => Ty::Void,
        };

        let Some(body) = tpl.body else {
            // extern 宣言 or 本体なし
            self.out.funcs.push(MonoFn {
                mangled: if tpl.export_abi { tpl.name.clone() } else { mangled.clone() },
                params: params_typed,
                ret,
                body: None,
                extern_abi: tpl.extern_abi.clone(),
                export_abi: tpl.export_abi,
                is_user_main: false,
            });
            return Ok(());
        };

        // 本体チェック
        self.cur_ret = ret.clone();
        self.scopes = vec![HashMap::new()];
        for (pn, pt) in &params_typed {
            self.scopes[0].insert(pn.clone(), LocalVar { ty: pt.clone(), mutable: pn == "self" });
        }
        self.unsafe_depth = 0;
        self.loop_depth = 0;

        let tbody = self.check_block(&body, Some(&ret))?;
        self.pop_scopes_to(1);

        let is_main = tpl.owner_type.is_none() && tpl.name == "main";

        if tpl.export_abi {
            self.out.exports.push(ExportSig {
                name: tpl.name.clone(),
                params: params_typed.clone(),
                ret: ret.clone(),
            });
        }

        self.out.funcs.push(MonoFn {
            mangled: if tpl.export_abi { tpl.name.clone() } else { mangled },
            params: params_typed,
            ret,
            body: Some(tbody),
            extern_abi: None,
            export_abi: tpl.export_abi,
            is_user_main: is_main,
        });
        Ok(())
    }

    fn finalize_exports(&mut self, file: &File) {
        let _ = file;
    }

    // ------------------------------------------------------------------
    // 型変換
    // ------------------------------------------------------------------
    fn conv_ty(&mut self, te: &TypeExpr, subst: &HashMap<String, Ty>, span: Span) -> Result<Ty, SemaError> {
        match te {
            TypeExpr::Ptr { elem, .. } => Ok(Ty::Ptr(Rc::new(self.conv_ty(elem, subst, span)?))),
            TypeExpr::Array { elem, len, .. } => {
                Ok(Ty::Array(Rc::new(self.conv_ty(elem, subst, span)?), *len))
            }
            TypeExpr::Named { name, args, span: sp } => {
                let prim = match name.as_str() {
                    "void" => Some(Ty::Void),
                    "bool" => Some(Ty::Bool),
                    "string" => Some(Ty::Str),
                    "i8" => Some(Ty::I8),
                    "i16" => Some(Ty::I16),
                    "i32" => Some(Ty::I32),
                    "i64" => Some(Ty::I64),
                    "u8" => Some(Ty::U8),
                    "u16" => Some(Ty::U16),
                    "u32" => Some(Ty::U32),
                    "u64" => Some(Ty::U64),
                    "usize" => Some(Ty::Usize),
                    "isize" => Some(Ty::Isize),
                    "f32" => Some(Ty::F32),
                    "f64" => Some(Ty::F64),
                    _ => None,
                };
                if let Some(t) = prim {
                    if !args.is_empty() {
                        return Err(SemaError { msg: format!("primitive type `{name}` takes no type arguments"), span: *sp });
                    }
                    return Ok(t);
                }
                if let Some(t) = subst.get(name) {
                    if !args.is_empty() {
                        return Err(SemaError { msg: format!("type parameter `{name}` cannot take arguments"), span: *sp });
                    }
                    return Ok(t.clone());
                }
                // ビルトイン / ユーザー定義
                if let Some((sid, _, arity)) = self.lookup_struct(name) {
                    let substs = self.convert_type_args(args, arity, subst, sp)?;
                    let mono = self.intern_struct(sid, substs)?;
                    return Ok(Ty::Struct(sid, {
                        // Ty::Struct の substs には intern 済み substs を入れる
                        self.out.structs[mono].substs.clone()
                    }));
                }
                if let Some((eid, _, arity)) = self.lookup_enum(name) {
                    let substs = self.convert_type_args(args, arity, subst, sp)?;
                    let mono = self.intern_enum(eid, substs.clone())?;
                    let _ = mono;
                    return Ok(Ty::Enum(eid, substs));
                }
                Err(SemaError { msg: format!("unknown type `{name}`"), span: *sp })
            }
        }
    }

    fn convert_type_args(
        &mut self,
        args: &[TypeExpr],
        arity: usize,
        subst: &HashMap<String, Ty>,
        span: &Span,
    ) -> Result<Vec<Ty>, SemaError> {
        if args.len() != arity {
            return Err(SemaError {
                msg: format!("type expects {} argument(s), got {}", arity, args.len()),
                span: *span,
            });
        }
        let mut out = Vec::new();
        for a in args {
            out.push(self.conv_ty(a, subst, *span)?);
        }
        Ok(out)
    }

    fn lookup_struct(&self, name: &str) -> Option<(usize, &StructDef, usize)> {
        if name == "List" {
            return Some((BUILTIN_LIST, &self.structs[BUILTIN_LIST], 1));
        }
        self.structs.iter().enumerate().find(|(_, d)| d.name == name).map(|(i, d)| (i, d, d.type_params.len()))
    }

    fn lookup_enum(&self, name: &str) -> Option<(usize, &EnumDef, usize)> {
        self.enums.iter().enumerate().find(|(_, d)| d.name == name).map(|(i, d)| (i, d, d.type_params.len()))
    }

    /// 構造体インスタンスのintern（モノモーフ化）
    fn intern_struct(&mut self, def_id: usize, substs: Vec<Ty>) -> Result<usize, SemaError> {
        // 既存チェック
        for (i, ms) in self.out.structs.iter().enumerate() {
            if ms.def_id == def_id && ms.substs == substs {
                return Ok(i);
            }
        }
        let def = self.structs[def_id].clone();
        let mangled = mangle(&def.name, &substs);

        let mut fields = Vec::new();
        if def_id == BUILTIN_LIST {
            // List<T>: 内部表現はランタイムが管理する不透明ポインタ
            fields.push(("__opaque".to_string(), Ty::Ptr(Rc::new(Ty::U8))));
        } else {
            let subst_map: HashMap<String, Ty> = def
                .type_params
                .iter()
                .cloned()
                .zip(substs.iter().cloned())
                .collect();
            for (fname, fty) in &def.fields {
                let t = self.conv_ty(fty, &subst_map, Span::new(0, 0))?;
                fields.push((fname.clone(), t));
            }
        }

        let idx = self.out.structs.len();
        self.out.structs.push(MonoStruct {
            def_id,
            substs: substs.clone(),
            mangled,
            fields,
            is_list: def_id == BUILTIN_LIST,
        });

        // List<T> のメソッド実装を投入
        if def_id == BUILTIN_LIST {
            for m in ["List::new", "List::push", "List::get", "List::set", "List::len"] {
                self.pending.push((m.to_string(), substs.clone()));
            }
        }
        Ok(idx)
    }

    fn intern_enum(&mut self, def_id: usize, substs: Vec<Ty>) -> Result<usize, SemaError> {
        for (i, me) in self.out.enums.iter().enumerate() {
            if me.def_id == def_id && me.substs == substs {
                return Ok(i);
            }
        }
        let def = self.enums[def_id].clone();
        let mangled = mangle(&def.name, &substs);
        let subst_map: HashMap<String, Ty> =
            def.type_params.iter().cloned().zip(substs.iter().cloned()).collect();
        let mut variants = Vec::new();
        for (vname, vtypes) in &def.variants {
            let mut pts = Vec::new();
            for vt in vtypes {
                pts.push(self.conv_ty(vt, &subst_map, Span::new(0, 0))?);
            }
            variants.push((vname.clone(), pts));
        }
        let idx = self.out.enums.len();
        self.out.enums.push(MonoEnum { def_id, substs, mangled, variants });
        Ok(idx)
    }

    // ------------------------------------------------------------------
    // 文・ブロック
    // ------------------------------------------------------------------
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn pop_scopes_to(&mut self, n: usize) {
        while self.scopes.len() > n {
            self.scopes.pop();
        }
    }
    fn lookup_var(&self, name: &str) -> Option<LocalVar> {
        for sc in self.scopes.iter().rev() {
            if let Some(lv) = sc.get(name) {
                return Some(LocalVar { ty: lv.ty.clone(), mutable: lv.mutable });
            }
        }
        None
    }
    fn declare_var(&mut self, name: String, ty: Ty, mutable: bool) {
        let last = self.scopes.last_mut().unwrap();
        last.insert(name, LocalVar { ty, mutable });
    }

    fn err(&mut self, msg: String, span: Span) {
        self.errors.push(SemaError { msg, span });
    }

    fn check_block(&mut self, b: &ngs_ast::Block, expect_tail: Option<&Ty>) -> Result<TBlock, SemaError> {
        self.push_scope();
        let scope_depth = self.scopes.len();
        let mut stmts = Vec::new();
        for s in &b.stmts {
            stmts.push(self.check_stmt(s)?);
        }
        let tail = match (&b.tail, expect_tail) {
            (Some(e), Some(ty)) => Some(Box::new(self.check_expr_expected(e, Some(ty))?)),
            (Some(e), None) => Some(Box::new(self.check_expr(e)?)),
            (None, _) => None,
        };
        // void 型の tail 式は値として使われないため、副作用文として扱う。
        // （例: `{ print(i) }` の print(i) は tail ではなく文になるべき）
        let tail = match tail {
            Some(t) if t.ty == Ty::Void => {
                stmts.push(TStmt::Expr(*t));
                None
            }
            other => other,
        };
        self.pop_scopes_to(scope_depth - 1);
        Ok(TBlock { stmts, tail })
    }

    fn check_stmt(&mut self, s: &Stmt) -> Result<TStmt, SemaError> {
        match s {
            Stmt::Let { name, ty, init, mutable, span, .. } => {
                let declared = match ty {
                    Some(t) => Some(self.conv_ty(t, &HashMap::new(), *span)?),
                    None => None,
                };
                let init_e = self.check_expr_expected(init, declared.as_ref())?;
                let var_ty = declared.unwrap_or_else(|| init_e.ty.clone());
                if var_ty == Ty::Void {
                    self.err("cannot bind variable to void expression".into(), *span);
                }
                self.declare_var(name.clone(), var_ty.clone(), *mutable);
                Ok(TStmt::Let(name.clone(), var_ty, init_e, *mutable))
            }
            Stmt::Assign { target, op, value, span } => {
                let texpr = self.check_expr(target)?;
                // 可変性チェック
                if let ExprKind::Path(path) = &target.kind {
                    if let Some(root) = path.first() {
                        if let Some(lv) = self.lookup_var(root) {
                            if !lv.mutable {
                                self.err(
                                    format!("cannot assign to immutable binding `{root}` (declare with `var`)"),
                                    *span,
                                );
                            }
                        }
                    }
                }
                let vexpr = self.check_expr_expected(value, Some(&texpr.ty))?;
                // 代入先は解決済みの LHS（ローカル / フィールド / インデックス / デリファレンス）のみ可
                if !matches!(
                    texpr.kind,
                    TExprKind::Local(_)
                        | TExprKind::Field { .. }
                        | TExprKind::Index { .. }
                        | TExprKind::Deref(_)
                ) {
                    self.err("invalid assignment target".into(), *span);
                }
                if texpr.ty.is_aggregate() && op.is_none() {
                    // 集約の全体コピーは許可（loweringでmemcpy）
                } else if !texpr.ty.is_numeric() && !texpr.ty.is_ptr() && texpr.ty != Ty::Bool && texpr.ty != Ty::Str {
                    if op.is_none() {
                        // ok: aggregate / rc handled above
                    }
                }
                if let Some(o) = op {
                    if !matches!(texpr.ty, Ty::F32 | Ty::F64) && !texpr.ty.is_int() {
                        self.err("compound assignment requires numeric type".into(), *span);
                    }
                    let _ = o;
                }
                Ok(TStmt::Assign(texpr, *op, vexpr))
            }
            Stmt::Expr(e) => Ok(TStmt::Expr(self.check_expr(e)?)),
            Stmt::Return { value, span } => {
                let v = match value {
                    Some(e) => {
                        let want = self.cur_ret.clone();
                        Some(self.check_expr_expected(e, Some(&want))?)
                    }
                    None => {
                        if self.cur_ret != Ty::Void {
                            return Err(SemaError {
                                msg: "`return` needs a value in this function".into(),
                                span: *span,
                            });
                        }
                        None
                    }
                };
                Ok(TStmt::Return(v))
            }
            Stmt::While { cond, body, .. } => {
                let c = self.check_expr_expected(cond, Some(&Ty::Bool))?;
                if c.ty != Ty::Bool {
                    self.err("while condition must be bool".into(), cond.span);
                }
                self.loop_depth += 1;
                let b = self.check_block(body, None)?;
                self.loop_depth -= 1;
                Ok(TStmt::While(c, b))
            }
            Stmt::ForRange { var, start, end, body, .. } => {
                let se = self.check_expr(start)?;
                if !se.ty.is_int() {
                    self.err("range-for bounds must be integers".into(), start.span);
                }
                let ee = self.check_expr_expected(end, Some(&se.ty))?;
                if ee.ty != se.ty {
                    self.err(
                        format!(
                            "range bound types differ: {} vs {}",
                            se.ty.display(),
                            ee.ty.display()
                        ),
                        end.span,
                    );
                }
                self.push_scope();
                self.loop_depth += 1;
                self.declare_var(var.clone(), se.ty.clone(), false);
                let b = self.check_block(body, None)?;
                self.loop_depth -= 1;
                self.pop_scope();
                Ok(TStmt::ForRange(var.clone(), se.ty.clone(), se, ee, b))
            }
            Stmt::ForC { init, cond, step, body, .. } => {
                self.push_scope();
                let i = match init {
                    Some(st) => {
                        let ts = self.check_stmt(st)?;
                        Some(Box::new(ts))
                    }
                    None => None,
                };
                let c = match cond {
                    Some(e) => {
                        let ce = self.check_expr_expected(e, Some(&Ty::Bool))?;
                        Some(ce)
                    }
                    None => None,
                };
                self.loop_depth += 1;
                let st = match step {
                    Some(e) => {
                        let ts = self.check_stmt(e)?;
                        Some(Box::new(ts))
                    }
                    None => None,
                };
                let b = self.check_block(body, None)?;
                self.loop_depth -= 1;
                self.pop_scope();
                Ok(TStmt::ForC(i, c, st, b))
            }
            Stmt::Break(_) => {
                if self.loop_depth == 0 {
                    self.err("`break` outside of loop".into(), s_span(s));
                }
                Ok(TStmt::Break)
            }
            Stmt::Continue(_) => {
                if self.loop_depth == 0 {
                    self.err("`continue` outside of loop".into(), s_span(s));
                }
                Ok(TStmt::Continue)
            }
        }
    }

    // ------------------------------------------------------------------
    // 式
    // ------------------------------------------------------------------
    fn check_expr(&mut self, e: &Expr) -> Result<TExpr, SemaError> {
        self.check_expr_expected(e, None)
    }

    /// addendum 3.4（病的ケースへのガードレール）:
    /// 検査の再帰深さに静的上限を設け、超えたら早期にエラーとして型注釈を要求する。
    fn check_expr_expected(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<TExpr, SemaError> {
        const MAX_EXPR_DEPTH: u32 = 128;
        if self.expr_depth >= MAX_EXPR_DEPTH {
            return Err(SemaError {
                msg: format!(
                    "expression nesting exceeds {MAX_EXPR_DEPTH} levels; add a type annotation or split the expression"
                ),
                span: e.span,
            });
        }
        self.expr_depth += 1;
        let r = self.check_expr_expected_inner(e, expected);
        self.expr_depth -= 1;
        let r = r?;
        // A4: 整数の縮小変換（i64→i32 等、ビットが失われる）は暗黙不可とし、`as` を要求する。
        if let Some(exp) = expected {
            if let (Some(ebits), Some(hbits)) = (exp.int_bits(), r.ty.int_bits()) {
                if ebits < hbits {
                    return Err(SemaError {
                        msg: format!(
                            "narrowing conversion requires `as`: from `{}` to `{}`",
                            r.ty.display(),
                            exp.display()
                        ),
                        span: e.span,
                    });
                }
            }
        }
        Ok(r)
    }

    fn check_expr_expected_inner(&mut self, e: &Expr, expected: Option<&Ty>) -> Result<TExpr, SemaError> {
        // リテラルは期待型への「適合」または明示的なエラーのどちらか。
        // 沈黙して別型のまま通すことはしない（let 注釈 / return 型との不一致検出）。
        let lit_mismatch = |me: &Self, got: &Ty| -> SemaError {
            let msg = match expected {
                Some(t) => format!("type mismatch: expected `{}`, found `{}`", t.display(), got.display()),
                None => "literal type mismatch".to_string(),
            };
            SemaError { msg, span: e.span }
        };
        match &e.kind {
            ExprKind::Int(v) => {
                let ty = match expected {
                    Some(t) if t.is_int() => t.clone(),
                    Some(Ty::F32) | Some(Ty::F64) => expected.cloned().unwrap(),
                    Some(other) => return Err(lit_mismatch(self, other)),
                    None => Ty::I32,
                };
                Ok(mk(Int(*v), ty, e.span))
            }
            ExprKind::Float(v) => {
                let ty = match expected {
                    Some(Ty::F32) => Ty::F32,
                    Some(t) if t.is_float() => Ty::F64,
                    Some(other) => return Err(lit_mismatch(self, other)),
                    None => Ty::F64,
                };
                Ok(mk(Float(*v), ty, e.span))
            }
            ExprKind::Bool(b) => {
                if let Some(t) = expected {
                    if *t != Ty::Bool {
                        return Err(lit_mismatch(self, &Ty::Bool));
                    }
                }
                Ok(mk(Bool(*b), Ty::Bool, e.span))
            }
            ExprKind::Null => {
                // null は unsafe ブロック内の生ポインタ型限定 (覇権戦略 3.3)
                self.require_unsafe(e.span, "null")?;
                Ok(mk(Null, Ty::Ptr(Rc::new(Ty::U8)), e.span))
            }
            ExprKind::Str(s) => {
                if let Some(t) = expected {
                    if *t != Ty::Str {
                        return Err(lit_mismatch(self, &Ty::Str));
                    }
                }
                Ok(mk(Str(s.clone()), Ty::Str, e.span))
            }
            ExprKind::FStr(_) => Err(SemaError {
                msg: "f-string is only valid directly inside print/println".into(),
                span: e.span,
            }),
            ExprKind::Path(path) => self.check_path(path, e.span, expected),
            ExprKind::Unary(op, inner) => {
                // &x と *p は特別扱い
                match op {
                    UnOp::AddrOf => {
                        let ie = self.check_expr(inner)?;
                        let ty = Ty::Ptr(Rc::new(ie.ty.clone()));
                        return Ok(mk(Unary(UnOp::AddrOf, Box::new(ie)), ty, e.span));
                    }
                    UnOp::Deref => {
                        let ie = self.check_expr(inner)?;
                        let pointee = match &ie.ty {
                            Ty::Ptr(t) => (**t).clone(),
                            Ty::RcT(t) => (**t).clone(),
                            other => {
                                return Err(SemaError {
                                    msg: format!("cannot dereference non-pointer type `{}`", other.display()),
                                    span: e.span,
                                })
                            }
                        };
                        self.require_unsafe(e.span, "pointer dereference")?;
                        return Ok(mk(Deref(Box::new(ie)), pointee, e.span));
                    }
                    UnOp::Neg => {
                        let ie = self.check_expr_expected(inner, expected)?;
                        let neg_ty = ie.ty.clone();
                        if !neg_ty.is_numeric() {
                            return Err(SemaError {
                                msg: format!("unary `-` requires numeric type, got `{}`", ie.ty.display()),
                                span: e.span,
                            });
                        }
                        return Ok(mk(Unary(UnOp::Neg, Box::new(ie)), neg_ty, e.span));
                    }
                    UnOp::Not => {
                        let ie = self.check_expr_expected(inner, Some(&Ty::Bool))?;
                        if ie.ty != Ty::Bool {
                            return Err(SemaError {
                                msg: "`!` requires bool".into(),
                                span: e.span,
                            });
                        }
                        return Ok(mk(Unary(UnOp::Not, Box::new(ie)), Ty::Bool, e.span));
                    }
                }
            }
            ExprKind::Binary(op, l, r) => self.check_binary(*op, l, r, e.span, expected),
            ExprKind::Cast(inner, ty) => {
                let ie = self.check_expr(inner)?;
                let to = self.conv_ty(ty, &HashMap::new(), e.span)?;
                validate_cast(&ie.ty, &to, e.span)?;
                Ok(mk(Cast(Box::new(ie)), to, e.span))
            }
            ExprKind::Call { callee, args } => {
                let exp2 = expected.cloned();
                self.check_call(callee, args, e.span, exp2.as_ref())
            }
            ExprKind::Index { base, index } => {
                let be = self.check_expr(base)?;
                let elem_ty = match &be.ty {
                    Ty::Array(t, _) => (**t).clone(),
                    Ty::Ptr(t) => {
                        self.require_unsafe(e.span, "raw pointer indexing")?;
                        (**t).clone()
                    }
                    other => {
                        return Err(SemaError {
                            msg: format!("cannot index into `{}`", other.display()),
                            span: e.span,
                        })
                    }
                };
                let ie = self.check_expr_expected(index, Some(&Ty::Usize))?;
                if !ie.ty.is_int() {
                    return Err(SemaError { msg: "index must be integer".into(), span: e.span });
                }
                let checked = !inside_unsafe_of(&be);
                Ok(TExpr { kind: TExprKind::Index { base: Box::new(be), index: Box::new(ie) }, ty: elem_ty, span: e.span, checked })
            }
            ExprKind::FieldAccess { base, field } => {
                let be = self.check_expr(base)?;
                let (idx, fty) = match &be.ty {
                    Ty::Struct(id, substs) => {
                        let mono = self.mono_struct_idx(*id, substs.clone())?;
                        let ms = self.out.structs[mono].clone();
                        if ms.is_list {
                            return Err(SemaError { msg: "List has no public fields; use methods".into(), span: e.span });
                        }
                        let pos = ms.fields.iter().position(|(n, _)| n == field).ok_or_else(|| SemaError {
                            msg: format!("struct `{}` has no field `{}`", ms.mangled, field),
                            span: e.span,
                        })?;
                        (pos, ms.fields[pos].1.clone())
                    }
                    other => {
                        return Err(SemaError {
                            msg: format!("`{}.{}`: `{}` is not a struct", field, field, other.display()),
                            span: e.span,
                        })
                    }
                };
                Ok(mk(Field { base: Box::new(be), index: idx }, fty, e.span))
            }
            ExprKind::StructLit { name, fields } => {
                let Some((sid, _, arity)) = self.lookup_struct(name) else {
                    return Err(SemaError { msg: format!("unknown struct `{name}`"), span: e.span });
                };
                if arity != 0 {
                    return Err(SemaError {
                        msg: format!("struct `{name}` requires type arguments in literal"),
                        span: e.span,
                    });
                }
                let mono = self.intern_struct(sid, vec![])?;
                let ms = self.out.structs[mono].clone();
                let mut tfields = Vec::new();
                let mut seen: HashSet<usize> = HashSet::new();
                for (fname, fexpr) in fields {
                    let pos = ms.fields.iter().position(|(n, _)| n == fname).ok_or_else(|| {
                        SemaError {
                            msg: format!("struct `{}` has no field `{}`", name, fname),
                            span: fexpr.span,
                        }
                    })?;
                    if !seen.insert(pos) {
                        self.err(format!("duplicate field `{fname}`"), fexpr.span);
                    }
                    let ft = ms.fields[pos].1.clone();
                    let fe = self.check_expr_expected(fexpr, Some(&ft))?;
                    if fe.ty != ft {
                        self.err(
                            format!(
                                "field `{}` type mismatch: expected `{}`, got `{}`",
                                fname,
                                ft.display(),
                                fe.ty.display()
                            ),
                            fexpr.span,
                        );
                    }
                    tfields.push((pos, fe));
                }
                if seen.len() != ms.fields.len() {
                    let missing: Vec<String> = ms
                        .fields
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !seen.contains(i))
                        .map(|(_, (n, _))| n.clone())
                        .collect();
                    self.err(
                        format!("missing field(s) in `{}` literal: {}", name, missing.join(", ")),
                        e.span,
                    );
                }
                return Ok(mk(
                    TExprKind::StructLit { mangled: ms.mangled.clone(), fields: tfields },
                    Ty::Struct(sid, vec![]),
                    e.span,
                ));
            }
            ExprKind::VariantCtor { enum_name, variant, payloads } => {
                self.build_variant_ctor_from_ast(enum_name.clone(), variant.clone(), payloads, e.span)
            }
            ExprKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, e.span, expected),
            ExprKind::If { cond, then_body, else_body } => {
                let c = self.check_cond(cond)?;
                let tb = self.check_block(then_body, expected)?;
                let eb = match else_body {
                    Some(el) => Some(Box::new(self.check_expr_expected(el, expected)?)),
                    None => None,
                };
                let ty = match expected {
                    Some(t) => t.clone(),
                    None => {
                        if eb.is_none() {
                            Ty::Void
                        } else {
                            tb.tail.as_ref().map(|t| t.ty.clone()).unwrap_or(Ty::Void)
                        }
                    }
                };
                Ok(mk(
                    TExprKind::If { cond: Box::new(c), then_body: tb, else_body: eb },
                    ty,
                    e.span,
                ))
            }
            ExprKind::BlockExpr(b) => {
                let tb = self.check_block(b, expected)?;
                let ty = tb.tail.as_ref().map(|t| t.ty.clone()).unwrap_or(Ty::Void);
                Ok(mk(TExprKind::Block(tb), ty, e.span))
            }
            ExprKind::UnsafeBlock(b) => {
                self.unsafe_depth += 1;
                let tb = self.check_block(b, expected)?;
                self.unsafe_depth -= 1;
                let ty = tb.tail.as_ref().map(|t| t.ty.clone()).unwrap_or(Ty::Void);
                Ok(mk(TExprKind::Block(tb), ty, e.span))
            }
            ExprKind::ArrayLit(elems) => {
                let elem_ty = match elems.first() {
                    Some(first) => self.check_expr(first)?.ty.clone(),
                    None => Ty::I32,
                };
                let mut tes = Vec::new();
                for el in elems {
                    let te = self.check_expr_expected(el, Some(&elem_ty))?;
                    if te.ty != elem_ty {
                        return Err(SemaError {
                            msg: format!(
                                "array element type mismatch: expected `{}`, got `{}`",
                                elem_ty.display(),
                                te.ty.display()
                            ),
                            span: el.span,
                        });
                    }
                    tes.push(te);
                }
                let ty = Ty::Array(Rc::new(elem_ty), tes.len() as u64);
                Ok(mk(ArrayLit(tes), ty, e.span))
            }
            ExprKind::Try(inner) => {
                let ie = self.check_expr(inner)?;
                // Result<T,E>? / Option<T>? → T（本体はloweringでmatchに展開）
                let ok_ty = match &ie.ty {
                    Ty::Enum(id, subs) => {
                        let name = self.enums[*id].name.clone();
                        if name == "Result" {
                            subs[0].clone()
                        } else if name == "Option" {
                            subs[0].clone()
                        } else {
                            return Err(SemaError {
                                msg: format!("`?` requires Result or Option, got `{}`", ie.ty.display()),
                                span: e.span,
                            });
                        }
                    }
                    other => {
                        return Err(SemaError {
                            msg: format!("`?` requires Result or Option, got `{}`", other.display()),
                            span: e.span,
                        })
                    }
                };
                Ok(mk(Try(Box::new(ie)), ok_ty, e.span))
            }
            ExprKind::Lambda { .. } => Err(SemaError {
                msg: "closures are planned for Stage 8 (mode B); not yet supported".into(),
                span: e.span,
            }),
            ExprKind::JsxProps(_) => Err(SemaError {
                msg: "JSX props are only valid as the 2nd argument of createElement".into(),
                span: e.span,
            }),
        }
    }

    fn check_cond(&mut self, e: &Expr) -> Result<TExpr, SemaError> {
        let c = self.check_expr_expected(e, Some(&Ty::Bool))?;
        if c.ty != Ty::Bool {
            return Err(SemaError {
                msg: format!("condition must be bool, got `{}`", c.ty.display()),
                span: e.span,
            });
        }
        Ok(c)
    }

    fn require_unsafe(&mut self, span: Span, what: &str) -> Result<(), SemaError> {
        if self.unsafe_depth > 0 {
            Ok(())
        } else {
            Err(SemaError {
                msg: format!("{what} is only allowed inside an `unsafe` block (spec 4.3)"),
                span,
            })
        }
    }

    fn check_binary(
        &mut self,
        op: BinOp,
        l: &Expr,
        r: &Expr,
        span: Span,
        _expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        use BinOp::*;
        // 片側がリテラルなら他方を先に評価して文脈型を与える
        let l_lit = is_plain_number_literal(l);
        let r_lit = is_plain_number_literal(r);
        let le;
        let re;
        if l_lit && !r_lit {
            re = self.check_expr(r)?;
            le = self.check_expr_expected(l, Some(&re.ty))?;
        } else {
            le = self.check_expr(l)?;
            re = self.check_expr_expected(r, Some(&le.ty))?;
        }

        let mkbin = |kind: TExprKind| TExpr { kind, ty: Ty::Bool, span, checked: false };

        match op {
            Add | Sub | Mul | Div | Mod => {
                if le.ty.is_float() && re.ty.is_float() {
                    let ty = le.ty.clone();
                    return Ok(TExpr {
                        kind: TExprKind::Binary(op, Box::new(le), Box::new(re)),
                        ty,
                        span,
                        checked: false,
                    });
                }
                if le.ty.is_int() && re.ty.is_int() {
                    if le.ty != re.ty {
                        return Err(SemaError {
                            msg: format!(
                                "integer type mismatch: `{}` vs `{}` (use `as` to convert)",
                                le.ty.display(),
                                re.ty.display()
                            ),
                            span,
                        });
                    }
                    let checked = self.unsafe_depth == 0;
                    let ty = le.ty.clone();
                    return Ok(TExpr {
                        kind: TExprKind::Binary(op, Box::new(le), Box::new(re)),
                        ty,
                        span,
                        checked,
                    });
                }
                Err(SemaError {
                    msg: format!(
                        "operator `{:?}` cannot be applied to `{}` and `{}`",
                        op,
                        le.ty.display(),
                        re.ty.display()
                    ),
                    span,
                })
            }
            Lt | Le | Gt | Ge | Eq | Neq => {
                if le.ty != re.ty {
                    // 数値リテラルのデフォルト不一致は許容しない（明示キャストを要求）
                    return Err(SemaError {
                        msg: format!(
                            "comparison between different types `{}` and `{}`",
                            le.ty.display(),
                            re.ty.display()
                        ),
                        span,
                    });
                }
                if !(le.ty.is_numeric() || le.ty == Ty::Bool || le.ty == Ty::Str || le.ty.is_ptr() || matches!(le.ty, Ty::Enum(..)))
                {
                    return Err(SemaError {
                        msg: format!("cannot compare values of type `{}`", le.ty.display()),
                        span,
                    });
                }
                Ok(mkbin(TExprKind::Binary(op, Box::new(le), Box::new(re))))
            }
            And | Or => {
                if le.ty != Ty::Bool || re.ty != Ty::Bool {
                    return Err(SemaError { msg: "`&&`/`||` require bool operands".into(), span });
                }
                Ok(mkbin(TExprKind::Binary(op, Box::new(le), Box::new(re))))
            }
        }
    }

    // ------------------------------------------------------------------
    // パス解決: 変数 / フィールド / メソッド呼び出し / 関連関数 /
    // enumバリアント / 組み込み
    // ------------------------------------------------------------------
    fn check_path(
        &mut self,
        path: &[String],
        span: Span,
        _expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        // 変数ルート
        if let Some(root) = path.first() {
            if let Some(lv) = self.lookup_var(root) {
                return self.check_member_chain(ExprKind::Path(vec![root.clone()]), lv.ty.clone(), span, &path[1..]);
            }
        }
        match path.len() {
            1 => {
                let name = &path[0];
                if self.fns.contains_key(name) {
                    let tpl = &self.fns[name];
                    if tpl.body.is_some() && !tpl.type_params.is_empty() {
                        return Err(SemaError {
                            msg: format!("generic function `{name}` needs type inference from call arguments"),
                            span,
                        });
                    }
                    return Err(SemaError {
                        msg: format!("function `{name}` used as a value; add a call `()`", name = name),
                        span,
                    });
                }
                Err(SemaError { msg: format!("unknown identifier `{}`", name), span })
            }
            2 => {
                // Type.associated または Enum.Variant
                let tn = &path[0];
                let second = &path[1];
                if let Some(&(eid, ..)) = self.lookup_enum(tn).as_ref().map(|x| x) {
                    if let Some(vi) = self.enums[eid].variants.iter().position(|(vn, _)| vn == second) {
                        let _ = vi;
                        return Err(SemaError {
                            msg: format!(
                                "variant `{}.{}` constructed as value; add parentheses payload",
                                tn, second
                            ),
                            span,
                        });
                    }
                }
                let key = method_key(tn, second);
                if self.fns.contains_key(&key) {
                    let tpl = self.fns[&key].clone();
                    if tpl.is_method {
                        return Err(SemaError {
                            msg: format!("`{}.{}` is a method; call it on a value", tn, second),
                            span,
                        });
                    }
                    if !tpl.type_params.is_empty() {
                        return Err(SemaError {
                            msg: format!("associated function `{}.{}` is generic", tn, second),
                            span,
                        });
                    }
                    // シグネチャだけ確認して呼び出しはCall側で
                    return Err(SemaError {
                        msg: format!("associated function `{}.{}` must be called", tn, second),
                        span,
                    });
                }
                Err(SemaError { msg: format!("unknown path `{}.{}`", tn, second), span })
            }
            _ => Err(SemaError { msg: "unsupported path length".into(), span }),
        }
    }

    /// 変数ルートからのメンバアクセス連鎖: a.b.c(...) 等
    fn check_member_chain(
        &mut self,
        _root_kind: ExprKind,
        mut cur_ty: Ty,
        span: Span,
        rest: &[String],
    ) -> Result<TExpr, SemaError> {
        let root = match &_root_kind {
            ExprKind::Path(p) => p[0].clone(),
            _ => unreachable!(),
        };
        let mut base = mk(Local(root), cur_ty.clone(), span);
        for (i, seg) in rest.iter().enumerate() {
            let last = i + 1 == rest.len();
            // フィールド?
            if let Ty::Struct(sid, substs) = &cur_ty {
                let mono = self.mono_struct_idx(*sid, substs.clone())?;
                let ms = self.out.structs[mono].clone();
                if !ms.is_list {
                    if let Some(pos) = ms.fields.iter().position(|(n, _)| n == seg) {
                        let fty = ms.fields[pos].1.clone();
                        base = mk(
                            Field { base: Box::new(base), index: pos },
                            fty.clone(),
                            span,
                        );
                        cur_ty = fty;
                        continue;
                    }
                }
            }
            // メソッド呼び出し（最終セグメントのみ）
            if last {
                return self.resolve_method_call(base, &cur_ty, seg, span);
            }
            return Err(SemaError {
                msg: format!("no member `{}` on type `{}`", seg, cur_ty.display()),
                span,
            });
        }
        Ok(base)
    }

    fn resolve_method_call(
        &mut self,
        recv: TExpr,
        recv_ty: &Ty,
        method: &str,
        span: Span,
    ) -> Result<TExpr, SemaError> {
        // 組み込みメソッド
        match recv_ty {
            Ty::Struct(BUILTIN_LIST, substs) => match method {
                "push" => {
                    let et = substs[0].clone();
                    return Ok(make_intrinsic2(
                        Intrinsic::ListPush,
                        recv,
                        et.clone(),
                        et,
                        span,
                    ));
                }
                "get" => {
                    let et = substs[0].clone();
                    let _ = et;
                    return Err(SemaError {
                        msg: "List.get takes an index argument".into(),
                        span,
                    });
                }
                "len" => {
                    return Ok(mk(Call(Callee::Intrinsic(Intrinsic::ListLen), vec![recv]), Ty::Usize, span));
                }
                _ => {}
            },
            Ty::RcT(inner) => match method {
                "get" | "value" => {
                    return Ok(mk(Deref(Box::new(recv)), (**inner).clone(), span));
                }
                _ => {}
            },
            _ => {}
        }
        Err(SemaError {
            msg: format!("no method `{}` on type `{}`", method, recv_ty.display()),
            span,
        })
    }

    fn mono_struct_idx(&mut self, def_id: usize, substs: Vec<Ty>) -> Result<usize, SemaError> {
        for (i, ms) in self.out.structs.iter().enumerate() {
            if ms.def_id == def_id && ms.substs == substs {
                return Ok(i);
            }
        }
        self.intern_struct(def_id, substs)
    }

    // ------------------------------------------------------------------
    // 呼び出し
    // ------------------------------------------------------------------
    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        // JSX props は createElement の第2引数でのみ有効
        if let ExprKind::JsxProps(attrs) = &callee.kind {
            let _ = attrs;
            return Err(SemaError { msg: "invalid JSX position".into(), span });
        }

        // 組み込み関数
        if let ExprKind::Path(path) = &callee.kind {
            match path.iter().map(|x| x.as_str()).collect::<Vec<_>>().as_slice() {
                ["print"] | ["println"] => {
                    let newline = path[0] == "println";
                    if args.len() != 1 {
                        return Err(SemaError { msg: "print takes exactly 1 argument".into(), span });
                    }
                    // f-string は print/println 限定で print-time 展開
                    if let ExprKind::FStr(parts) = &args[0].kind {
                        let mut tparts = Vec::with_capacity(parts.len());
                        for part in parts {
                            match part {
                                FStringPart::Text(s) => tparts.push(TFStringPart::Text(s.clone())),
                                FStringPart::Expr(e) => {
                                    let te = self.check_expr(e)?;
                                    if !(te.ty.is_numeric() || te.ty == Ty::Bool || te.ty == Ty::Str) {
                                        return Err(SemaError {
                                            msg: format!("cannot interpolate `{}`", te.ty.display()),
                                            span,
                                        });
                                    }
                                    tparts.push(TFStringPart::Expr(te));
                                }
                            }
                        }
                        return Ok(mk(
                            Call(Callee::Intrinsic(Intrinsic::PrintFStr { newline, parts: tparts }), vec![]),
                            Ty::Void,
                            span,
                        ));
                    }
                    let a = self.check_expr(&args[0])?;
                    let vt = a.ty.clone();
                    if !(vt.is_numeric() || vt == Ty::Bool || vt == Ty::Str) {
                        return Err(SemaError {
                            msg: format!("print cannot display `{}`", vt.display()),
                            span,
                        });
                    }
                    return Ok(mk(
                        Call(Callee::Intrinsic(Intrinsic::Print { newline, value_ty: vt }), vec![a]),
                        Ty::Void,
                        span,
                    ));
                }
                ["panic"] => {
                    if args.len() != 1 {
                        return Err(SemaError { msg: "panic takes a message".into(), span });
                    }
                    let a = self.check_expr(&args[0])?;
                    if a.ty != Ty::Str {
                        return Err(SemaError { msg: "panic message must be a string".into(), span });
                    }
                    return Ok(mk(Call(Callee::Intrinsic(Intrinsic::Panic), vec![a]), Ty::Void, span));
                }
                ["abort"] => {
                    return Ok(mk(Call(Callee::Intrinsic(Intrinsic::Abort), vec![]), Ty::Void, span));
                }
                ["len"] => {
                    if args.len() != 1 {
                        return Err(SemaError { msg: "len takes exactly 1 argument".into(), span });
                    }
                    let a = self.check_expr(&args[0])?;
                    match a.ty {
                        Ty::Array(..) | Ty::Str => {}
                        _ => {
                            return Err(SemaError {
                                msg: format!("len requires array or string, got `{}`", a.ty.display()),
                                span,
                            })
                        }
                    }
                    return Ok(mk(Call(Callee::Intrinsic(Intrinsic::Len), vec![a]), Ty::Usize, span));
                }
                ["createElement"] => return self.check_create_element(args, span),
                _ => {}
            }
        }

        // メソッド呼び出し: recv.method(args...)
        if let ExprKind::FieldAccess { base, field } = &callee.kind {
            // ベースがローカル変数ルートのパスならメソッド解決
            if let ExprKind::Path(p) = &base.kind {
                if p.len() >= 1 {
                    if let Some(root) = p.first() {
                        if let Some(lv) = self.lookup_var(root) {
                            // 中間フィールドアクセスがある場合: a.b.method(...)
                            let recv = if p.len() == 1 {
                                mk(Local(root.clone()), lv.ty.clone(), base.span)
                            } else {
                                self.check_member_chain_no_call(root, lv.ty.clone(), base.span, &p[1..], field)?
                            };
                            let recv_ty = recv.ty.clone();
                            return self.call_method_with_args(recv, &recv_ty, field, args, span);
                        }
                    }
                }
            } else {
                // 複雑な式のメソッド呼び出し foo().bar(...)
                let be = self.check_expr(base)?;
                let recv_ty = be.ty.clone();
                return self.call_method_with_args(be, &recv_ty, field, args, span);
            }
        }

        // 通常の関数呼び出し
        match &callee.kind {
            ExprKind::Path(path) => {
                match path.len() {
                    1 => {
                        let name = &path[0];
                        // enumバリアント直接生成: Circle(1.0)
                        if let Some(enum_id) = self.find_variant_owner(name) {
                            return self.build_variant_ctor(enum_id, name.clone(), args, span, expected);
                        }
                        let key = name.clone();
                        self.instantiate_call(&key, args, span)
                    }
                    2 => {
                        let tn = &path[0];
                        let second = &path[1];
                        // 変数ルートのメソッド呼び出し: list.push(...)
                        if let Some(lv) = self.lookup_var(tn) {
                            let recv = mk(Local(tn.clone()), lv.ty.clone(), callee.span);
                            let recv_ty = lv.ty.clone();
                            return self.call_method_with_args(recv, &recv_ty, second, args, span);
                        }
                        // ビルトイン: List<T>.new() / Rc<T>.new(v)
                        if tn == "List" && second == "new" {
                            return self.build_list_new(args, span, expected);
                        }
                        if tn == "Rc" && second == "new" {
                            if args.len() != 1 {
                                return Err(SemaError { msg: "Rc.new takes 1 value".into(), span });
                            }
                            let v = self.check_expr(&args[0])?;
                            let vty = v.ty.clone();
                            if vty == Ty::Void || vty.is_aggregate() {
                                return Err(SemaError {
                                    msg: format!("Rc requires a scalar payload, got `{}`", vty.display()),
                                    span,
                                });
                            }
                            return Ok(mk(
                                Call(Callee::Intrinsic(Intrinsic::RcNew), vec![v]),
                                Ty::RcT(Rc::new(vty)),
                                span,
                            ));
                        }
                        // Enum.Variant(payload)
                        if let Some((eid, _, _)) = self.lookup_enum(tn) {
                            if self.enums[eid].variants.iter().any(|(vn, _)| vn == second) {
                                return self.build_variant_ctor(eid, second.clone(), args, span, expected);
                            }
                        }
                        // Type.associated(args)
                        let key = method_key(tn, second);
                        if self.fns.contains_key(&key) {
                            let tpl = self.fns[&key].clone();
                            if tpl.is_method {
                                return Err(SemaError {
                                    msg: format!("`{}.{}` requires a receiver", tn, second),
                                    span,
                                });
                            }
                            return self.instantiate_call(&key, args, span);
                        }
                        Err(SemaError {
                            msg: format!("unknown function `{}.{}`", tn, second),
                            span,
                        })
                    }
                    _ => Err(SemaError { msg: "unsupported call target".into(), span }),
                }
            }
            _ => Err(SemaError {
                msg: "function pointers are not supported yet".into(),
                span: callee.span,
            }),
        }
    }

    fn check_member_chain_no_call(
        &mut self,
        root: &str,
        ty: Ty,
        span: Span,
        mid: &[String],
        _last_field: &str,
    ) -> Result<TExpr, SemaError> {
        let mut cur = mk(Local(root.to_string()), ty, span);
        for seg in mid {
            let curt = cur.ty.clone();
            cur = self.resolve_field(cur, &curt, seg, span)?;
        }
        Ok(cur)
    }

    fn resolve_field(&mut self, base: TExpr, ty: &Ty, field: &str, span: Span) -> Result<TExpr, SemaError> {
        if let Ty::Struct(sid, substs) = ty {
            let mono = self.mono_struct_idx(*sid, substs.clone())?;
            let ms = self.out.structs[mono].clone();
            if ms.is_list {
                return Err(SemaError { msg: "List has no public fields".into(), span });
            }
            let pos = ms.fields.iter().position(|(n, _)| n == field).ok_or_else(|| SemaError {
                msg: format!("no field `{}` on `{}`", field, ms.mangled),
                span,
            })?;
            let fty = ms.fields[pos].1.clone();
            Ok(mk(Field { base: Box::new(base), index: pos }, fty, span))
        } else {
            Err(SemaError {
                msg: format!("`{}` is not a struct", ty.display()),
                span,
            })
        }
    }

    fn call_method_with_args(
        &mut self,
        recv: TExpr,
        recv_ty: &Ty,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<TExpr, SemaError> {
        // 組み込み
        match recv_ty {
            Ty::Struct(BUILTIN_LIST, substs) => match method {
                "push" => {
                    if args.len() != 1 {
                        return Err(SemaError { msg: "List.push takes 1 argument".into(), span });
                    }
                    let mut et = substs[0].clone();
                    let a = self.check_expr(&args[0])?;
                    if Self::is_pending_list_elem(&et) {
                        // 遅延束縛: ローカル変数の型を確定させる
                        et = a.ty.clone();
                        if let TExprKind::Local(root) = &recv.kind {
                            for sc in self.scopes.iter_mut().rev() {
                                if let Some(lv) = sc.get_mut(root) {
                                    lv.ty = Ty::Struct(BUILTIN_LIST, vec![et.clone()]);
                                    break;
                                }
                            }
                        }
                        self.intern_struct(BUILTIN_LIST, vec![et.clone()])?;
                    } else {
                        let expect2 = et.clone();
                        let a2 = self.check_expr_expected(&args[0], Some(&expect2))?;
                        let _ = a2;
                        if a.ty != et {
                            return Err(SemaError {
                                msg: format!(
                                    "List element type is `{}`, got `{}`",
                                    et.display(),
                                    a.ty.display()
                                ),
                                span,
                            });
                        }
                    }
                    return Ok(mk(
                        Call(Callee::Intrinsic(Intrinsic::ListPush), vec![recv, a]),
                        Ty::Void,
                        span,
                    ));
                }
                "get" => {
                    if args.len() != 1 {
                        return Err(SemaError { msg: "List.get takes 1 index argument".into(), span });
                    }
                    let i = self.check_expr_expected(&args[0], Some(&Ty::Usize))?;
                    if !i.ty.is_int() {
                        return Err(SemaError { msg: "index must be integer".into(), span });
                    }
                    let et = substs[0].clone();
                    return Ok(mk(
                        Call(Callee::Intrinsic(Intrinsic::ListGet), vec![recv, i]),
                        et,
                        span,
                    ));
                }
                "set" => {
                    if args.len() != 2 {
                        return Err(SemaError { msg: "List.set takes (index, value)".into(), span });
                    }
                    let i = self.check_expr_expected(&args[0], Some(&Ty::Usize))?;
                    let et = substs[0].clone();
                    let v = self.check_expr_expected(&args[1], Some(&et))?;
                    return Ok(mk(
                        Call(Callee::Intrinsic(Intrinsic::ListSet), vec![recv, i, v]),
                        Ty::Void,
                        span,
                    ));
                }
                "len" => {
                    if !args.is_empty() {
                        return Err(SemaError { msg: "List.len takes no arguments".into(), span });
                    }
                    return Ok(mk(Call(Callee::Intrinsic(Intrinsic::ListLen), vec![recv]), Ty::Usize, span));
                }
                _ => {}
            },
            Ty::RcT(inner) => match method {
                "get" | "value" => {
                    if !args.is_empty() {
                        return Err(SemaError { msg: "Rc.get takes no arguments".into(), span });
                    }
                    return Ok(mk(Deref(Box::new(recv)), (**inner).clone(), span));
                }
                _ => {}
            },
            Ty::Enum(id, substs) if self.enums[*id].name == "Result" => match method {
                "is_ok" | "is_err" => {
                    let _ = substs;
                    return Err(SemaError {
                        msg: "use `match` on the Result instead (methods is_ok/is_err land in Stage 5)".into(),
                        span,
                    });
                }
                _ => {}
            },
            _ => {}
        }

        // ユーザー定義メソッド
        let owner = match recv_ty {
            Ty::Struct(id, _) => Some(self.structs[*id].name.clone()),
            Ty::Enum(id, _) => Some(self.enums[*id].name.clone()),
            Ty::RcT(inner) => match inner.as_ref() {
                Ty::Struct(id, _) => Some(format!("Rc<{}>", self.structs[*id].name)),
                _ => None,
            },
            _ => None,
        };
        let Some(owner) = owner else {
            return Err(SemaError {
                msg: format!("no method `{}` on type `{}`", method, recv_ty.display()),
                span,
            });
        };
        // Rc<T>.foo → T::foo への委譲はしない（明示的にderef）
        let key = method_key(&owner, method);
        if !self.fns.contains_key(&key) {
            return Err(SemaError {
                msg: format!("no method `{}` on type `{}`", method, recv_ty.display()),
                span,
            });
        }
        let tpl = self.fns[&key].clone();
        if !tpl.is_method {
            return Err(SemaError {
                msg: format!("`{}.{}` is an associated function; call as `{}.{}(...)`", owner, method, owner, method),
                span,
            });
        }

        // 受信型からsubsts導出（例: recv: Stack<i32> → owner Stack, substs [i32]）
        let substs: Vec<Ty> = match &recv_ty {
            Ty::Struct(_, s) => s.clone(),
            Ty::Enum(_, s) => s.clone(),
            _ => vec![],
        };
        let mangled = mangle(&format!("{}", key.replace("::", ".")), &substs);
        if !self.done_fns.contains(&mangled) {
            self.pending.push((key.clone(), substs.clone()));
        }
        // 引数チェック
        let mut targs = Vec::new();
        let params_after_self = &tpl.params[1..];
        if args.len() != params_after_self.len() {
            return Err(SemaError {
                msg: format!(
                    "`{}.{}` takes {} argument(s), got {}",
                    owner,
                    method,
                    params_after_self.len(),
                    args.len()
                ),
                span,
            });
        }
        // パラメータ型の実体化（subst適用）→ 簡易: conv via subst map
        let subst_map: HashMap<String, Ty> = tpl
            .type_params
            .iter()
            .cloned()
            .zip(substs.iter().cloned())
            .chain(
                // メソッド独自の型パラメータは未対応（推論が必要）— エラーにする
                vec![]
            )
            .collect();
        for (i, (_, pty)) in params_after_self.iter().enumerate() {
            let pt = self.conv_ty(pty, &subst_map, span)?;
            let a = self.check_expr_expected(&args[i], Some(&pt))?;
            if a.ty != pt {
                // ジェネリック受信で残ったGeneric型はここでは検査できないため緩める
                if !matches!(pt, Ty::Generic(_)) {
                    return Err(SemaError {
                        msg: format!(
                            "argument {} type mismatch: expected `{}`, got `{}`",
                            i + 1,
                            pt.display(),
                            a.ty.display()
                        ),
                        span,
                    });
                }
            }
            targs.push(a);
        }
        // 戻り値型
        let ret_ty = match &tpl.ret {
            Some(rt) => {
                let t = self.conv_ty(rt, &subst_map, span)?;
                if matches!(t, Ty::Generic(_)) {
                    Ty::Void // 推論不能（将来改善）
                } else {
                    t
                }
            }
            None => Ty::Void,
        };
        // 受信者を第1引数（self）として呼び出し式を組み立てる
        let mut full_args = Vec::with_capacity(targs.len() + 1);
        full_args.push(recv);
        full_args.extend(targs);
        Ok(mk(Call(Callee::Direct(mangled), full_args), ret_ty, span))
    }

    fn build_list_new(
        &mut self,
        _args: &[Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        // 要素型は注釈、または最初の push 実引数から遅延束縛される
        const PENDING: &str = "\u{1}list";
        let elem = match expected {
            Some(Ty::Struct(BUILTIN_LIST, subs)) => subs[0].clone(),
            _ => Ty::Generic(PENDING.to_string()),
        };
        Ok(mk(
            Call(Callee::Intrinsic(Intrinsic::ListNew), vec![]),
            Ty::Struct(BUILTIN_LIST, vec![elem]),
            span,
        ))
    }

    fn is_pending_list_elem(ty: &Ty) -> bool {
        matches!(ty, Ty::Generic(g) if g == "\u{1}list")
    }

    fn find_variant_owner(&self, variant: &str) -> Option<usize> {
        let owners = self.variant_owners.get(variant)?;
        if owners.len() == 1 {
            Some(owners[0])
        } else {
            None // 曖昧 → 修飾を要求
        }
    }

    fn build_variant_ctor(
        &mut self,
        enum_id: usize,
        variant: String,
        args: &[Expr],
        span: Span,
        expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        // 注釈された期待型が同じenumなら、その型引数を優先
        if let Some(Ty::Enum(eid, subs)) = expected {
            if *eid == enum_id {
                return self.emit_variant(enum_id, subs, &variant, args, span);
            }
        }
        let arity = self.enums[enum_id].type_params.len();
        if arity != 0 {
            // バリアントpayloadからsubsts推論（Result/Option等ビルトイン含む）
            let def = self.enums[enum_id].clone();
            let mut substs: Vec<Option<Ty>> = vec![None; arity];
            let vi = def.variants.iter().position(|(vn, _)| *vn == variant).unwrap();
            let payload_tys = &def.variants[vi].1;
            if args.len() != payload_tys.len() {
                return Err(SemaError {
                    msg: format!("variant `{}` takes {} payload value(s)", variant, payload_tys.len()),
                    span,
                });
            }
            for (i, pt) in payload_tys.iter().enumerate() {
                if let TypeExpr::Named { name, args: targs, .. } = pt {
                    if targs.is_empty() {
                        if let Some(pos) = def.type_params.iter().position(|tp| tp == name) {
                            let guessed = self.check_expr(&args[i])?;
                            if substs[pos].is_some() && substs[pos] != Some(guessed.ty.clone()) {
                                return Err(SemaError {
                                    msg: format!("conflicting type inference for `{}`", def.type_params[pos]),
                                    span,
                                });
                            }
                            substs[pos] = Some(guessed.ty.clone());
                            continue;
                        }
                    }
                }
                let _ = self.check_expr(&args[i])?;
            }
            let substs: Vec<Ty> = substs
                .into_iter()
                .enumerate()
                .map(|(i, o)| o.unwrap_or_else(|| {
                    // 未決定はvoid（エラーケース）。Result<i32, ?>等は明示型注釈が必要
                    let _ = i;
                    Ty::Void
                }))
                .collect();
            return self.emit_variant(enum_id, &substs, &variant, args, span);
        }
        self.emit_variant(enum_id, &[], &variant, args, span)
    }

    fn emit_variant(
        &mut self,
        enum_id: usize,
        substs: &[Ty],
        variant: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<TExpr, SemaError> {
        let mono = self.intern_enum(enum_id, substs.to_vec())?;
        let me = self.out.enums[mono].clone();
        let vi = me.variants.iter().position(|(vn, _)| vn == variant).unwrap();
        let payloads = me.variants[vi].1.clone();
        if args.len() != payloads.len() {
            return Err(SemaError {
                msg: format!("variant `{}` takes {} payload value(s)", variant, payloads.len()),
                span,
            });
        }
        let mut tp = Vec::new();
        for (i, pt) in payloads.iter().enumerate() {
            let a = self.check_expr_expected(&args[i], Some(pt))?;
            if a.ty != *pt {
                return Err(SemaError {
                    msg: format!(
                        "payload {} type mismatch: expected `{}`, got `{}`",
                        i + 1,
                        pt.display(),
                        a.ty.display()
                    ),
                    span,
                });
            }
            tp.push(a);
        }
        Ok(mk(
            TExprKind::VariantCtor { mangled: me.mangled.clone(), variant: vi, payloads: tp },
            Ty::Enum(enum_id, substs.to_vec()),
            span,
        ))
    }

    fn instantiate_call(&mut self, key: &str, args: &[Expr], span: Span) -> Result<TExpr, SemaError> {
        let tpl = self.fns.get(key).cloned().ok_or_else(|| SemaError {
            msg: format!("unknown function `{key}`"),
            span,
        })?;
        if tpl.body.is_none() && tpl.extern_abi.is_none() {
            return Err(SemaError { msg: format!("function `{key}` has no body"), span });
        }
        if args.len() != tpl.params.len() {
            return Err(SemaError {
                msg: format!("`{}` takes {} argument(s), got {}", tpl.name, tpl.params.len(), args.len()),
                span,
            });
        }

        // Pass 1: 全引数を素のまま検査し、構造照合で型パラメータを束縛
        let mut arg_tys: Vec<Ty> = Vec::with_capacity(args.len());
        let mut subst: HashMap<String, Ty> = HashMap::new();
        for (i, a) in args.iter().enumerate() {
            let te = self.check_expr(a)?;
            arg_tys.push(te.ty.clone());
            let (_, pty) = &tpl.params[i];
            if !unify_structure(pty, &te.ty, &tpl.type_params, &mut subst) {
                // 構造不一致は具象型同士の可能性 → Pass 2 で厳密検査される
            }
        }
        // 未決定の型パラメータ
        for tp in &tpl.type_params {
            if !subst.contains_key(tp) {
                return Err(SemaError {
                    msg: format!(
                        "cannot infer type parameter `{}` of `{}`; annotate the variable",
                        tp, tpl.name
                    ),
                    span,
                });
            }
        }
        let ordered: Vec<Ty> = tpl.type_params.iter().map(|t| subst[t].clone()).collect();

        let mangled = mangle(&display_key(key), &ordered);
        if tpl.body.is_some() && !self.done_fns.contains(&mangled) {
            self.pending.push((key.to_string(), ordered.clone()));
        }

        // Pass 2: 具象パラメータ型で引数を厳密検査
        let mut targs = Vec::new();
        for (i, (_, pty)) in tpl.params.iter().enumerate() {
            let pt = self.conv_ty_lenient(pty, &subst, span)?;
            let a = self.check_expr_expected(&args[i], Some(&pt))?;
            if a.ty != pt {
                return Err(SemaError {
                    msg: format!(
                        "argument {} of `{}`: expected `{}`, got `{}`",
                        i + 1,
                        tpl.name,
                        pt.display(),
                        a.ty.display()
                    ),
                    span,
                });
            }
            targs.push(a);
        }

        let ret_ty = match &tpl.ret {
            Some(rt) => {
                let t = self.conv_ty_lenient(rt, &subst, span)?;
                if matches!(t, Ty::Generic(_)) {
                    return Err(SemaError {
                        msg: format!(
                            "return type of `{}` could not be inferred; annotate the variable",
                            tpl.name
                        ),
                        span,
                    });
                }
                t
            }
            None => Ty::Void,
        };

        if tpl.extern_abi.is_some() || tpl.body.is_none() {
            return Ok(mk(Call(Callee::Extern(tpl.name.clone()), targs), ret_ty, span));
        }
        Ok(mk(Call(Callee::Direct(mangled), targs), ret_ty, span))
    }

    fn conv_ty_no_generics(&mut self, te: &TypeExpr, span: Span) -> Result<Ty, SemaError> {
        self.conv_ty(te, &HashMap::new(), span)
    }

    /// Generic が残ることを許す変換（引数再検査用）
    fn conv_ty_lenient(&mut self, te: &TypeExpr, subst: &HashMap<String, Ty>, span: Span) -> Result<Ty, SemaError> {
        self.conv_ty(te, subst, span)
    }

    // ------------------------------------------------------------------
    // createElement(tag, props, children...) — JSX糖衣展開先
    // ------------------------------------------------------------------
    fn build_variant_ctor_from_ast(
        &mut self,
        enum_name: Option<String>,
        variant: String,
        payloads: &[Expr],
        span: Span,
    ) -> Result<TExpr, SemaError> {
        let eid = match enum_name {
            Some(en) => {
                let Some((e, _, _)) = self.lookup_enum(&en) else {
                    return Err(SemaError { msg: format!("unknown enum `{en}`"), span });
                };
                e
            }
            None => {
                let Some(e) = self.find_variant_owner(&variant) else {
                    return Err(SemaError {
                        msg: format!(
                            "ambiguous or unknown variant `{variant}`; qualify as Enum.Variant"
                        ),
                        span,
                    });
                };
                e
            }
        };
        self.build_variant_ctor(eid, variant, payloads, span, None)
    }

    fn check_create_element(&mut self, args: &[Expr], span: Span) -> Result<TExpr, SemaError> {
        if args.len() < 2 {
            return Err(SemaError { msg: "createElement requires (tag, props)".into(), span });
        }
        // 第1引数: タグ文字列 or コンポーネント関数
        match &args[0].kind {
            ExprKind::Str(_) => {
                let t = self.check_expr(&args[0])?;
                let _ = t;
            }
            ExprKind::Path(p) if p.len() == 1 => {
                // コンポーネント関数参照（存在チェックのみ・Stage 8で本格対応）
                let name = &p[0];
                if !self.fns.contains_key(name) {
                    return Err(SemaError {
                        msg: format!("JSX component `{name}` is not defined"),
                        span: args[0].span,
                    });
                }
            }
            _ => {
                return Err(SemaError {
                    msg: "JSX tag must be a lowercase string or component name".into(),
                    span: args[0].span,
                })
            }
        }
        // 第2引数: props
        let mut tprops: Vec<(String, TExpr)> = Vec::new();
        match &args[1].kind {
            ExprKind::JsxProps(attrs) => {
                for (k, v) in attrs {
                    let tv = self.check_expr(v)?;
                    tprops.push((k.clone(), tv));
                }
            }
            _ => return Err(SemaError { msg: "createElement 2nd argument must be JSX props".into(), span }),
        }
        let props_expr = mk(TExprKind::Props(tprops), Ty::Props, args[1].span);

        let mut targs = vec![mk(Str(match &args[0].kind {
            ExprKind::Str(s) => s.clone(),
            ExprKind::Path(p) => p[0].clone(),
            _ => unreachable!(),
        }), Ty::Str, args[0].span), props_expr];

        for child in &args[2..] {
            let tc = self.check_expr(child)?;
            targs.push(tc);
        }
        Ok(mk(
            Call(Callee::Intrinsic(Intrinsic::PropsNew), targs),
            Ty::Void,
            span,
        ))
    }

    // ------------------------------------------------------------------
    // match
    // ------------------------------------------------------------------
    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        span: Span,
        expected: Option<&Ty>,
    ) -> Result<TExpr, SemaError> {
        let scr = self.check_expr(scrutinee)?;
        let scr_ty = scr.ty.clone();
        let mut tarms = Vec::new();
        let mut covered_variants: HashSet<usize> = HashSet::new();
        let mut has_wildcard = false;

        for arm in arms {
            match &arm.pattern.kind {
                PatternKind::Wildcard => {
                    has_wildcard = true;
                    let b = self.check_expr_expected(&arm.body, expected)?;
                    tarms.push(TArm { pattern: TPattern::Wildcard, body: b });
                }
                PatternKind::Int(v) => {
                    if !scr_ty.is_int() {
                        return Err(SemaError {
                            msg: format!("integer pattern against `{}`", scr_ty.display()),
                            span: arm.pattern.span,
                        });
                    }
                    if scr_ty != Ty::I32 && scr_ty.is_int() {
                        // リテラル側はscr型に合わせる
                    }
                    let _ = v;
                    let b = self.check_expr_expected(&arm.body, expected)?;
                    tarms.push(TArm { pattern: TPattern::Int(*v), body: b });
                }
                PatternKind::Bool(v) => {
                    if scr_ty != Ty::Bool {
                        return Err(SemaError {
                            msg: "bool pattern requires bool scrutinee".into(),
                            span: arm.pattern.span,
                        });
                    }
                    let b = self.check_expr_expected(&arm.body, expected)?;
                    tarms.push(TArm { pattern: TPattern::Bool(*v), body: b });
                }
                PatternKind::Str(s) => {
                    if scr_ty != Ty::Str {
                        return Err(SemaError {
                            msg: "string pattern requires string scrutinee".into(),
                            span: arm.pattern.span,
                        });
                    }
                    let b = self.check_expr_expected(&arm.body, expected)?;
                    tarms.push(TArm { pattern: TPattern::Str(s.clone()), body: b });
                }
                PatternKind::Variant { enum_name, variant, bindings } => {
                    let eid = match enum_name {
                        Some(en) => {
                            let Some((e, _, _)) = self.lookup_enum(en) else {
                                return Err(SemaError {
                                    msg: format!("unknown enum `{en}`"),
                                    span: arm.pattern.span,
                                });
                            };
                            e
                        }
                        None => match scr_ty {
                            Ty::Enum(e, _) => e,
                            ref other => {
                                return Err(SemaError {
                                    msg: format!("variant pattern against non-enum `{}`", other.display()),
                                    span: arm.pattern.span,
                                })
                            }
                        },
                    };
                    if !matches!(scr_ty, Ty::Enum(id, _) if id == eid) {
                        return Err(SemaError {
                            msg: format!("pattern enum does not match scrutinee type `{}`", scr_ty.display()),
                            span: arm.pattern.span,
                        });
                    }
                    let Ty::Enum(_, substs) = scr_ty.clone() else { unreachable!() };
                    let mono = self.intern_enum(eid, substs.clone())?;
                    let me = self.out.enums[mono].clone();
                    let Some(vi) = me.variants.iter().position(|(vn, _)| vn == variant) else {
                        return Err(SemaError {
                            msg: format!("enum `{}` has no variant `{}`", me.mangled, variant),
                            span: arm.pattern.span,
                        });
                    };
                    let payload_tys = me.variants[vi].1.clone();
                    if bindings.len() != payload_tys.len() {
                        return Err(SemaError {
                            msg: format!(
                                "variant `{}` has {} payload(s), pattern binds {}",
                                variant,
                                payload_tys.len(),
                                bindings.len()
                            ),
                            span: arm.pattern.span,
                        });
                    }
                    self.push_scope();
                    let mut tb = Vec::new();
                    for (bn, bt) in bindings.iter().zip(payload_tys.iter()) {
                        self.declare_var(bn.clone(), bt.clone(), false);
                        tb.push((bn.clone(), bt.clone()));
                    }
                    let b = self.check_expr_expected(&arm.body, expected)?;
                    self.pop_scope();
                    covered_variants.insert(vi);
                    tarms.push(TArm {
                        pattern: TPattern::Variant { mangled: me.mangled.clone(), variant: vi, bindings: tb },
                        body: b,
                    });
                }
            }
        }

        // 網羅性
        if let Ty::Enum(eid, substs) = &scr_ty {
            let mono = self.intern_enum(*eid, substs.clone())?;
            let nv = self.out.enums[mono].variants.len();
            if !has_wildcard && covered_variants.len() < nv {
                let me = &self.out.enums[mono];
                let missing: Vec<String> = me
                    .variants
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !covered_variants.contains(i))
                    .map(|(_, (n, _))| n.clone())
                    .collect();
                self.err(
                    format!("non-exhaustive match: missing variant(s) {}", missing.join(", ")),
                    span,
                );
            }
        } else if !has_wildcard {
            // スカラーmatchはワイルドカード必須
            self.err("non-exhaustive match: add a `_` arm".to_string(), span);
        }

        let ty = match expected {
            Some(t) => t.clone(),
            None => tarms.first().map(|a| a.body.ty.clone()).unwrap_or(Ty::Void),
        };
        Ok(mk(TExprKind::Match { scrutinee: Box::new(scr), arms: tarms }, ty, span))
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn mk(kind: TExprKind, ty: Ty, span: Span) -> TExpr {
    TExpr { kind, ty, span, checked: false }
}

fn make_intrinsic2(i: Intrinsic, a: TExpr, bty: Ty, ret: Ty, span: Span) -> TExpr {
    let _ = bty;
    mk(Call(Callee::Intrinsic(i), vec![a, TExpr { kind: TExprKind::UninitPlaceholder, ty: ret.clone(), span, checked: false }]), ret, span)
}

fn s_span(s: &Stmt) -> Span {
    match s {
        Stmt::Break(sp) | Stmt::Continue(sp) => *sp,
        _ => Span::new(0, 0),
    }
}

fn is_plain_number_literal(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Int(_) | ExprKind::Float(_))
}

fn inside_unsafe_of(_e: &TExpr) -> bool {
    false // ポインタindexは既にsemaでunsafe要求済み。境界チェックは配列のみで使う
}

/// パラメータ型 te と実引数型 ty を構造的に照合し、型パラメータの束縛を得る
fn unify_structure(
    te: &TypeExpr,
    ty: &Ty,
    params: &[String],
    subst: &mut HashMap<String, Ty>,
) -> bool {
    match te {
        TypeExpr::Named { name, args, .. } => {
            if args.is_empty() {
                if let Some(_) = params.iter().position(|p| p == name) {
                    if let Some(prev) = subst.get(name) {
                        return *prev == *ty;
                    }
                    subst.insert(name.clone(), ty.clone());
                    return true;
                }
            }
            match (name.as_str(), ty) {
                (_, _) => {}
            }
            // 構造が一致するか確認して子を照合
            match ty {
                Ty::Ptr(t) => {
                    if let TypeExpr::Ptr { elem, .. } = te {
                        return unify_structure(elem, t, params, subst);
                    }
                    false
                }
                Ty::Array(t, n) => {
                    if let TypeExpr::Array { elem, len, .. } = te {
                        return len == n && unify_structure(elem, t, params, subst);
                    }
                    false
                }
                Ty::Struct(sid, subs) | Ty::Enum(sid, subs) => {
                    if let TypeExpr::Named { name: tn, args: targs, .. } = te {
                        // 名前一致チェックは呼び出し側で簡易に行うため、ここでは引数個数のみ
                        if targs.len() == subs.len() {
                            for (a, b) in targs.iter().zip(subs.iter()) {
                                if !unify_structure(a, b, params, subst) {
                                    return false;
                                }
                            }
                            let _ = sid;
                            return true;
                        }
                    }
                    false
                }
                Ty::RcT(t) => {
                    if let TypeExpr::Named { name: rn, args: rargs, .. } = te {
                        if rn == "Rc" && rargs.len() == 1 {
                            return unify_structure(&rargs[0], t, params, subst);
                        }
                    }
                    false
                }
                _ => false,
            }
        }
        TypeExpr::Ptr { elem, .. } => {
            if let Ty::Ptr(t) = ty {
                unify_structure(elem, t, params, subst)
            } else {
                false
            }
        }
        TypeExpr::Array { elem, .. } => {
            if let Ty::Array(t, _) = ty {
                unify_structure(elem, t, params, subst)
            } else {
                false
            }
        }
    }
}

fn validate_cast(from: &Ty, to: &Ty, span: Span) -> Result<(), SemaError> {
    let ok = match (from, to) {
        (a, b) if a.is_numeric() && b.is_numeric() => true,
        (Ty::Bool, b) if b.is_int() => true,
        (a, Ty::Bool) if a.is_int() => true,
        (Ty::Ptr(_), b) if b.is_int() => true,
        (a, Ty::Ptr(_)) if a.is_int() => true,
        (Ty::Ptr(_), Ty::Ptr(_)) => true,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(SemaError {
            msg: format!("unsupported cast from `{}` to `{}`", from.display(), to.display()),
            span,
        })
    }
}

fn display_key(key: &str) -> String {
    key.replace("::", ".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ngs_parser::parse_source;

    fn check_src(src: &str) -> Result<TypedProgram, Vec<SemaError>> {
        let file = parse_source(src, "test.ngs").map_err(|e| {
            vec![SemaError { msg: format!("{e}"), span: Span { lo: 0, hi: 0 } }]
        })?;
        check(&file)
    }

    #[test]
    fn normal_expressions_are_unaffected() {
        let ok = check_src("fn main() { val x = ((((1 + 2) * 3)) - 4); println(x); }");
        assert!(ok.is_ok(), "{:?}", ok.err().map(|es| es.iter().map(|e| e.msg.clone()).collect::<Vec<_>>()));
    }
}
