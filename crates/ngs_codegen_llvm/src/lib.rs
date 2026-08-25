//! NGS-IR → LLVM IR (.ll テキスト) コード生成。
//!
//! 設計方針:
//! - inkwell 等を使わずテキスト形式の LLVM IR を直接出力する。
//!   clang / llc があればそのまま実行ファイルへできるため、ツールチェーン間の
//!   ABI 差異に左右されず、デバッグも容易。
//! - 構造体・enum・配列・文字列はすべて「実体へのポインタ」として扱う
//!   （NGS-IR lowering の規約と一致）。LLVM 側には名前付き型を導入せず、
//!   アドレス演算はすべて `getelementptr inbounds i8` のバイト単位で行う。
//! - 集約型（struct/enum/array）を返す関数は sret 規約を用いる:
//!   隠し第1引数（呼び出し側が確保したバッファへの ptr）へ書き込み、
//!   LLVM レベルでは void を返す。Str は実体が共有可能なため ptr を直接返す。
//! - フィールドオフセットは round_up(size(field), 8) 累積、enum は
//!   {tag:u64 @0, payload0@8, payload1@16}、Rc は {count@0, size@8, data@16}。
//! - C エクスポート関数は mangled 名で定義し、export シンボル名の薄いラッパを
//!   追加する（内部呼び出しと C 側呼び出しの両方を壊さないため）。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use ngs_ir::{CastKind, Const, Inst, IrBin, IrFunction, IrPred, IrProgram, IrType, Term, NO_V};

// ---------------------------------------------------------------------------
// 型レイアウト（サイズ計算は ngs_ir::IrProgram に集約）
// ---------------------------------------------------------------------------

/// 値が「実体へのポインタ」で表される型か
pub fn is_val_ptr(t: &IrType) -> bool {
    IrProgram::is_val_ptr(t)
}

fn is_signed(t: &IrType) -> bool {
    matches!(t, IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::Isize)
}

/// LLVM 上の値型
fn ll_type(t: &IrType) -> &'static str {
    match t {
        IrType::Void => "void",
        IrType::Bool => "i1",
        IrType::I8 | IrType::U8 => "i8",
        IrType::I16 | IrType::U16 => "i16",
        IrType::I32 | IrType::U32 | IrType::F32 => "i32",
        IrType::I64 | IrType::U64 | IrType::Usize | IrType::Isize => "i64",
        IrType::F64 => "double",
        IrType::Str | IrType::Ptr(_) | IrType::Struct(_) | IrType::Enum(_) | IrType::Array(..) => "ptr",
    }
}

fn ll_align(t: &IrType) -> u64 {
    if is_val_ptr(t) {
        8
    } else {
        match t {
            IrType::Bool | IrType::I8 | IrType::U8 => 1,
            IrType::I16 | IrType::U16 => 2,
            IrType::F32 | IrType::I32 | IrType::U32 => 4,
            _ => 8,
        }
    }
}

fn ll_name(name: &str) -> String {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ok && !name.chars().next().unwrap().is_ascii_digit() {
        format!("@{name}")
    } else {
        // 識別子として使えない文字はエスケープして引用符で囲む
        // （メソッド名などドット入りの mangled 名向け）
        format!("@\"{name}\"")
    }
}

fn ll_label(label: &str) -> String {
    let ok = label
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_' || c == '.')
        .unwrap_or(false)
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$');
    if ok {
        label.to_string()
    } else {
        format!("b.{}", crc(&label))
    }
}

fn crc(s: &str) -> u64 {
    let mut h = 5381u64;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    h & 0xFFFFF
}

fn f64_hex(v: f64) -> String {
    format!("0x{:016X}", v.to_bits())
}

// ---------------------------------------------------------------------------
// ランタイム関数シグネチャ（runtime.c と一致させること）
// ---------------------------------------------------------------------------

const P: &str = "ptr";
pub const RUNTIME_SIGS: &[(&str, &[&str], &str)] = &[
    ("__ngs_print_str", &[P, "i64"], "void"),
    ("__ngs_println_str", &[P, "i64"], "void"),
    ("__ngs_print_i64", &["i64"], "void"),
    ("__ngs_println_i64", &["i64"], "void"),
    ("__ngs_print_f64", &["double"], "void"),
    ("__ngs_println_f64", &["double"], "void"),
    ("__ngs_print_bool", &["i8"], "void"),
    ("__ngs_println_bool", &["i8"], "void"),
    ("__ngs_panic", &[P, "i64"], "void"),
    ("__ngs_abort", &[], "void"),
    ("__ngs_str_eq", &[P, P], "i8"),
    ("__ngs_str_to_i64", &[P], "i64"),
    ("__ngs_str_to_f64", &[P], "double"),
    ("__ngs_list_new", &["i64"], P),
    ("__ngs_list_push", &[P, "i64"], P),
    ("__ngs_list_len", &[P], "i64"),
    ("__ngs_list_at", &[P, "i64"], P),
    ("__ngs_list_free", &[P], "void"),
    ("__ngs_rc_new", &["i64"], P),
    ("__ngs_rc_inc", &[P], "void"),
    ("__ngs_rc_dec", &[P], "void"),
    ("__ngs_box_i64", &["i64"], "i64"),
    ("__ngs_box_f64", &["double"], "i64"),
    ("__ngs_box_bool", &["i8"], "i64"),
    ("__ngs_box_str", &[P], "i64"),
    ("__ngs_box_ptr", &[P], "i64"),
    ("__ngs_props_new", &[], P),
    ("__ngs_props_tag", &[P, P, "i64"], "void"),
    ("__ngs_props_set", &[P, P, "i64", "i64"], "void"),
    ("__ngs_props_add_child", &[P, "i64"], "void"),
];

// ---------------------------------------------------------------------------
// 生成器
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LlvmOptions {
    /// @main ラッパを生成する（実行ファイル用）
    pub gen_main_wrapper: bool,
    /// ターゲット triple（Noneなら省略＝ホスト既定）
    pub target_triple: Option<String>,
}

impl Default for LlvmOptions {
    fn default() -> Self {
        LlvmOptions { gen_main_wrapper: true, target_triple: None }
    }
}

#[derive(Debug)]
pub struct CodegenError(pub String);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CodegenError {}

/// 呼び出し先シグネチャ（LLVM レベル）
struct CalleeSig {
    /// LLVM 引数型（sret は含まない）
    params: Vec<String>,
    /// LLVM 戻り値型（sret 時は void）
    ret: String,
    /// 集約返却のため隠し先頭引数を持つか
    sret: bool,
}

struct FuncGen<'a> {
    prog: &'a IrProgram,
    out: String,
    tmp: usize,
    /// V id -> オペランド文字列（"%v12" や "@.strc.3"）
    vals: HashMap<usize, String>,
    /// V id -> 論理型
    tys: HashMap<usize, IrType>,
    /// 参照した外部シンボル（後段で declare を生成）
    externals: HashSet<String>,
    /// 現在の関数の sret パラメータ名（集約返却時のみ Some）
    sret: Option<String>,
    /// main ラッパとのシンボル衝突回避用リネーム (元名, 定義名)
    main_rename: Option<(String, String)>,
}

pub fn generate(prog: &IrProgram, opts: &LlvmOptions) -> Result<String, CodegenError> {
    let mut header = String::new();
    if let Some(triple) = &opts.target_triple {
        writeln!(header, "target triple = \"{triple}\"").ok();
    }
    writeln!(header, "; generated by nagiscript (NGS-IR -> LLVM IR)").ok();
    writeln!(header, "source_filename = \"nagiscript\"").ok();
    writeln!(header).ok();

    // 文字列グローバル（実体バイト列 + {data,len} セル）
    let mut globals = String::new();
    for (id, g) in prog.strings.iter().enumerate() {
        let bytes = escape_bytes(g.content.as_bytes());
        writeln!(
            globals,
            "@.str.{id} = private unnamed_addr constant [{} x i8] c\"{}\"",
            g.content.len(),
            bytes
        )
        .ok();
        writeln!(
            globals,
            "@.strc.{id} = private unnamed_addr constant {{ ptr, i64 }} {{ ptr @.str.{id}, i64 {} }}",
            g.content.len()
        )
        .ok();
    }

    // export 名の割り当て（定義済み C-ABI 関数 -> export シンボル）
    let mut export_map: HashMap<String, String> = HashMap::new();
    let mut used_exports: HashSet<usize> = HashSet::new();
    for f in &prog.funcs {
        if f.cconv_c && !f.is_decl {
            for (ei, (ename, eparams, eret)) in prog.exports.iter().enumerate() {
                if used_exports.contains(&ei) {
                    continue;
                }
                let psame = eparams.len() == f.params.len()
                    && eparams
                        .iter()
                        .zip(f.params.iter())
                        .all(|((_, ta), (_, tb))| ll_type(ta) == ll_type(tb));
                let rsame = match (eret.is_aggregate(), f.ret.is_aggregate()) {
                    (true, true) => true,
                    (false, false) => ll_type(eret) == ll_type(&f.ret),
                    _ => false,
                };
                if psame && rsame {
                    export_map.insert(f.name.clone(), ename.clone());
                    used_exports.insert(ei);
                    break;
                }
            }
        }
    }

    let mut body = String::new();
    let mut all_externals: HashSet<String> = HashSet::new();

    // ユーザ main が素の "main" 名の場合、main ラッパとシンボルが衝突するため
    // 定義側をリネームする
    let main_rename: Option<(String, String)> = if opts.gen_main_wrapper {
        prog.main_name
            .as_ref()
            .filter(|m| ll_name(m) == "@main")
            .map(|_| ("main".to_string(), "__ngs_user_main".to_string()))
    } else {
        None
    };

    for f in &prog.funcs {
        if f.blocks.is_empty() {
            continue; // 宣言のみ
        }
        let mut g = FuncGen {
            prog,
            out: String::new(),
            tmp: 0,
            vals: HashMap::new(),
            tys: HashMap::new(),
            externals: HashSet::new(),
            sret: None,
            main_rename: main_rename.clone(),
        };
        g.gen_function(f)?;
        body.push_str(&g.out);
        body.push('\n');
        all_externals.extend(g.externals.drain());
    }

    // 宣言（IR上の外部宣言 + 参照したランタイム/組込みシンボル）
    let mut decl_lines: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in &prog.funcs {
        if f.blocks.is_empty() && seen.insert(f.name.clone()) {
            decl_lines.push(decl_line(&f.name, f));
        }
    }
    let mut ext: Vec<&String> = all_externals.iter().collect();
    ext.sort();
    for name in ext {
        if seen.insert(name.clone()) {
            if let Some(d) = external_decl(name) {
                decl_lines.push(d);
            }
        }
    }
    let mut decls = String::new();
    for d in decl_lines {
        writeln!(decls, "{d}").ok();
    }

    // C エクスポートの薄いラッパ（内部呼び出しは mangled 名で通す）。
    // mangled 名と export 名が一致する場合は定義がそのままシンボルになるため不要。
    let mut wrappers = String::new();
    for f in &prog.funcs {
        if f.blocks.is_empty() {
            continue;
        }
        if let Some(exp) = export_map.get(&f.name) {
            if exp != &f.name {
                write_export_wrapper(&mut wrappers, f, exp);
            }
        }
    }

    // main ラッパ（実行ファイル用）
    let mut main_wrap = String::new();
    if opts.gen_main_wrapper {
        let mn = prog
            .main_name
            .as_ref()
            .ok_or_else(|| CodegenError("program has no `main` function".into()))?;
        let mf = prog
            .funcs
            .iter()
            .find(|f| &f.name == mn && !f.blocks.is_empty())
            .ok_or_else(|| CodegenError(format!("main `{mn}` has no definition")))?;
        if mf.ret.is_aggregate() {
            return Err(CodegenError("`main` cannot return an aggregate value".into()));
        }
        let sym = match &main_rename {
            Some((from, to)) if *from == mf.name => ll_name(to),
            _ => ll_name(&mf.name),
        };
        writeln!(main_wrap, "define i32 @main() {{").ok();
        match ll_type(&mf.ret) {
            "void" => {
                writeln!(main_wrap, "  call void {sym}()").ok();
                writeln!(main_wrap, "  ret i32 0").ok();
            }
            "i32" => {
                writeln!(main_wrap, "  %r = call i32 {sym}()").ok();
                writeln!(main_wrap, "  ret i32 %r").ok();
            }
            rt @ ("i1" | "i8" | "i16" | "i64") => {
                writeln!(main_wrap, "  %r = call {rt} {sym}()").ok();
                if matches!(mf.ret, IrType::I8 | IrType::I16) && is_signed(&mf.ret) {
                    writeln!(main_wrap, "  %w = sext {rt} %r to i32").ok();
                } else if rt == "i64" {
                    writeln!(main_wrap, "  %w = trunc i64 %r to i32").ok();
                } else {
                    writeln!(main_wrap, "  %w = zext {rt} %r to i32").ok();
                }
                writeln!(main_wrap, "  ret i32 %w").ok();
            }
            other => {
                return Err(CodegenError(format!(
                    "`main` must return void or an integer type, got `{other}`"
                )));
            }
        }
        writeln!(main_wrap, "}}").ok();
    }

    Ok(format!("{header}{globals}\n{decls}\n{body}\n{wrappers}{main_wrap}"))
}

/// IR 上の宣言（extern / proto）用 declare 行
fn decl_line(sym: &str, f: &IrFunction) -> String {
    let sret = f.ret.is_aggregate();
    let mut ps: Vec<&str> = Vec::new();
    if sret {
        ps.push("ptr");
    }
    for (_, t) in &f.params {
        ps.push(ll_type(t));
    }
    let rt = if sret { "void" } else { ll_type(&f.ret) };
    format!("declare {rt} {}({})", ll_name(sym), ps.join(", "))
}

/// 外部シンボル（ランタイム / LLVM 組込み / libm）の declare 行
fn external_decl(name: &str) -> Option<String> {
    if let Some(inner) = name.strip_prefix("llvm.") {
        if inner.ends_with(".with.overflow") {
            // 例: llvm.sadd.with.overflow.iN
            let bits = name.rsplit('.').next()?;
            return Some(format!("declare {{ {bits}, i1 }} @{name}({bits}, {bits})"));
        }
        if name == "llvm.memcpy.p0.p0.i64" {
            return Some("declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)".into());
        }
        return None;
    }
    if let Some((_, ps, r)) = RUNTIME_SIGS.iter().find(|(n, _, _)| **n == *name) {
        return Some(format!("declare {r} {}({})", ll_name(name), ps.join(", ")));
    }
    if name == "fmod" {
        return Some("declare double @fmod(double, double)".into());
    }
    None
}

fn escape_bytes(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            b'\\' => s.push_str("\\\\"),
            b'"' => s.push_str("\\22"),
            0x20..=0x7E => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{:02X}", b);
            }
        }
    }
    s
}

/// C エクスポート用の転送ラッパ。内部コードは mangled 名を直接呼ぶため、
/// 定義本体は mangled 名のままにしてエクスポート名から中継する。
fn write_export_wrapper(out: &mut String, f: &IrFunction, exp: &str) {
    let inner = ll_name(&f.name);
    let sret = f.ret.is_aggregate();
    let llvm_ret = if sret { "void" } else { ll_type(&f.ret) };
    let mut ps: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if sret {
        ps.push("ptr %s".into());
        args.push("ptr %s".into());
    }
    for (i, (_, t)) in f.params.iter().enumerate() {
        ps.push(format!("{} %a{i}", ll_type(t)));
        args.push(format!("{} %a{i}", ll_type(t)));
    }
    writeln!(out, "define {llvm_ret} {}({}) {{", ll_name(exp), ps.join(", ")).ok();
    if sret || llvm_ret == "void" {
        writeln!(out, "  call void {inner}({})", args.join(", ")).ok();
        writeln!(out, "  ret void").ok();
    } else {
        writeln!(out, "  %r = call {llvm_ret} {inner}({})", args.join(", ")).ok();
        writeln!(out, "  ret {llvm_ret} %r").ok();
    }
    writeln!(out, "}}").ok();
}

impl<'a> FuncGen<'a> {
    fn fresh(&mut self) -> String {
        let n = self.tmp;
        self.tmp += 1;
        format!("%t{n}")
    }

    fn w(&mut self, line: &str) {
        self.out.push_str("  ");
        self.out.push_str(line);
        self.out.push('\n');
    }

    fn start_block(&mut self, label: &str) {
        let l = ll_label(label);
        writeln!(self.out, "{l}:").ok();
    }

    fn op(&self, v: usize) -> String {
        self.vals.get(&v).cloned().unwrap_or_else(|| format!("%v{v}"))
    }

    fn ty_of(&self, v: usize) -> IrType {
        self.tys.get(&v).cloned().unwrap_or(IrType::I64)
    }

    fn bind(&mut self, v: usize, ty: IrType, name: String) {
        self.vals.insert(v, name);
        self.tys.insert(v, ty);
    }

    fn gen_function(&mut self, f: &IrFunction) -> Result<(), CodegenError> {
        let sym = match &self.main_rename {
            Some((from, to)) if *from == f.name => ll_name(to),
            _ => ll_name(&f.name),
        };
        let sret = f.ret.is_aggregate();
        let llvm_ret = if sret { "void" } else { ll_type(&f.ret) };
        let mut ps: Vec<String> = Vec::new();
        if sret {
            ps.push("ptr %sret".into());
        }
        for (i, (_, t)) in f.params.iter().enumerate() {
            ps.push(format!("{} %v{i}", ll_type(t)));
        }
        writeln!(self.out, "define {llvm_ret} {sym}({}) {{", ps.join(", ")).ok();

        // パラメータ仮想レジスタ 0..n-1 をそのまま LLVM パラメータ名として使う
        for (i, (_, t)) in f.params.iter().enumerate() {
            self.bind(i, t.clone(), format!("%v{i}"));
        }
        if sret {
            self.sret = Some("%sret".into());
        }

        for block in &f.blocks {
            self.start_block(&block.label);
            for inst in &block.insts {
                self.gen_inst(inst)?;
            }
            match &block.term {
                Term::Ret(None) => self.w("ret void"),
                Term::Ret(Some(v)) => {
                    let ty = self.ty_of(*v);
                    let o = self.op(*v);
                    if ty.is_aggregate() {
                        // 呼び出し側提供バッファへ実体をコピーして終了
                        let sp = self
                            .sret
                            .clone()
                            .ok_or_else(|| CodegenError("aggregate return without sret slot".into()))?;
                        let sz = self.prog.size_of(&ty).max(1);
                        self.externals.insert("llvm.memcpy.p0.p0.i64".into());
                        self.w(&format!(
                            "call void @llvm.memcpy.p0.p0.i64(ptr {sp}, ptr {o}, i64 {sz}, i1 false)"
                        ));
                        self.w("ret void");
                    } else {
                        let t = ll_type(&ty);
                        self.w(&format!("ret {t} {o}"));
                    }
                }
                Term::Br(l) => self.w(&format!("br label %{}", ll_label(l))),
                Term::CondBr(c, a, b) => {
                    let co = self.op(*c);
                    self.w(&format!(
                        "br i1 {}, label %{}, label %{}",
                        co,
                        ll_label(a),
                        ll_label(b)
                    ));
                }
                Term::Unreachable => self.w("unreachable"),
            }
        }
        writeln!(self.out, "}}").ok();
        Ok(())
    }

    fn widen_to_i64(&mut self, v: usize) -> Result<(String, bool), CodegenError> {
        let ty = self.ty_of(v);
        let o = self.op(v);
        if matches!(ty, IrType::I64 | IrType::U64 | IrType::Usize | IrType::Isize) {
            return Ok((o, is_signed(&ty)));
        }
        let r = self.fresh();
        if matches!(ty, IrType::Bool) {
            self.w(&format!("{r} = zext i1 {o} to i64"));
            return Ok((r, false));
        }
        let bits = ty.bits();
        if is_signed(&ty) {
            self.w(&format!("{r} = sext i{bits} {o} to i64"));
        } else {
            self.w(&format!("{r} = zext i{bits} {o} to i64"));
        }
        Ok((r, is_signed(&ty)))
    }

    fn gen_inst(&mut self, inst: &Inst) -> Result<(), CodegenError> {
        match inst {
            Inst::Const { dst, val } => match val {
                Const::Int(n) => {
                    let r = self.fresh();
                    self.w(&format!("{r} = add i64 0, {n}"));
                    self.bind(*dst, IrType::I64, r);
                }
                Const::Float(x) => {
                    let r = self.fresh();
                    self.w(&format!("{r} = fadd double 0.0, {}", f64_hex(*x)));
                    self.bind(*dst, IrType::F64, r);
                }
                Const::Bool(b) => {
                    let r = self.fresh();
                    self.w(&format!("{r} = xor i1 false, {}", if *b { 1 } else { 0 }));
                    self.bind(*dst, IrType::Bool, r);
                }
                Const::Str(id) => {
                    // {data,len} 定数セルのアドレスがそのまま Str 値
                    self.bind(*dst, IrType::Str, format!("@.strc.{id}"));
                }
                Const::NullPtr => {
                    let r = self.fresh();
                    self.w(&format!("{r} = inttoptr i64 0 to ptr"));
                    self.bind(*dst, IrType::Ptr(std::rc::Rc::new(IrType::U8)), r);
                }
            },
            Inst::Alloca { dst, ty } => {
                let r = format!("%v{dst}");
                if ty.is_aggregate() || matches!(ty, IrType::Str) {
                    // 生バッファを確保し、そのアドレスを実体値とする
                    // （StructLit / ArrayLit / VariantCtor / copy_agg 宛先など）
                    let sz = self.prog.size_of(ty).max(1);
                    self.w(&format!("{r} = alloca [{sz} x i8], align 16"));
                } else {
                    // スカラーまたはポインタセル
                    self.w(&format!("{r} = alloca {}, align {}", ll_type(ty), ll_align(ty)));
                }
                self.bind(*dst, ty.clone(), r);
            }
            Inst::Load { dst, addr, ty } => {
                let a = self.op(*addr);
                let t = ll_type(ty);
                let r = format!("%v{dst}");
                self.w(&format!("{r} = load {t}, ptr {a}, align {}", ll_align(ty)));
                self.bind(*dst, ty.clone(), r);
            }
            Inst::Store { addr, val, ty } => {
                let a = self.op(*addr);
                let vo = self.op(*val);
                // 定数は論理型へ合わせる（i64 定数を小さい整数型へ trunc など）
                let vt = self.ty_of(*val);
                let t = ll_type(ty);
                let vfinal = if t != ll_type(&vt) && t != "ptr" && ll_type(&vt) != "ptr" {
                    self.coerce(&vo, ll_type(&vt), t, is_signed(&vt))
                } else {
                    vo
                };
                self.w(&format!("store {t} {vfinal}, ptr {a}, align {}", ll_align(ty)));
            }
            Inst::BinOp { dst, op, ty, a, b } => {
                let ao = self.op(*a);
                let bo = self.op(*b);
                let lt = ll_type(ty);
                let mnemonic = match (op, ty.is_float()) {
                    (IrBin::Add, false) => "add",
                    (IrBin::Sub, false) => "sub",
                    (IrBin::Mul, false) => "mul",
                    (IrBin::Div, false) => {
                        if is_signed(ty) {
                            "sdiv"
                        } else {
                            "udiv"
                        }
                    }
                    (IrBin::Mod, false) => {
                        if is_signed(ty) {
                            "srem"
                        } else {
                            "urem"
                        }
                    }
                    (IrBin::Add, true) => "fadd",
                    (IrBin::Sub, true) => "fsub",
                    (IrBin::Mul, true) => "fmul",
                    (IrBin::Div, true) => "fdiv",
                    (IrBin::Mod, true) => "frem",
                };
                let (ao, bo) =
                    self.match_operand_types(&ao, &bo, &self.ty_of(*a), &self.ty_of(*b), ty)?;
                let r = format!("%v{dst}");
                self.w(&format!("{r} = {mnemonic} {lt} {ao}, {bo}"));
                self.bind(*dst, ty.clone(), r);
            }
            Inst::CmpOp { dst, pred, ty, a, b } => {
                let ao = self.op(*a);
                let bo = self.op(*b);
                let (ao, bo) =
                    self.match_operand_types(&ao, &bo, &self.ty_of(*a), &self.ty_of(*b), ty)?;
                let pred_s = if ty.is_float() {
                    match pred {
                        IrPred::Eq => "oeq",
                        IrPred::Ne => "one",
                        IrPred::Lt => "olt",
                        IrPred::Le => "ole",
                        IrPred::Gt => "ogt",
                        IrPred::Ge => "oge",
                    }
                } else {
                    match pred {
                        IrPred::Eq => "eq",
                        IrPred::Ne => "ne",
                        IrPred::Lt => {
                            if is_signed(ty) {
                                "slt"
                            } else {
                                "ult"
                            }
                        }
                        IrPred::Le => {
                            if is_signed(ty) {
                                "sle"
                            } else {
                                "ule"
                            }
                        }
                        IrPred::Gt => {
                            if is_signed(ty) {
                                "sgt"
                            } else {
                                "ugt"
                            }
                        }
                        IrPred::Ge => {
                            if is_signed(ty) {
                                "sge"
                            } else {
                                "uge"
                            }
                        }
                    }
                };
                let kind = if ty.is_float() { "fcmp" } else { "icmp" };
                let r = format!("%v{dst}");
                self.w(&format!("{r} = {kind} {pred_s} {} {ao}, {bo}", ll_type(ty)));
                self.bind(*dst, IrType::Bool, r);
            }
            Inst::CheckedBin { dst_val, dst_ovf, op, ty, a, b } => {
                let bits = ty.bits();
                let s = if is_signed(ty) { "s" } else { "u" };
                let mn = match op {
                    IrBin::Add => "add",
                    IrBin::Sub => "sub",
                    IrBin::Mul => "mul",
                    _ => return Err(CodegenError("checked op only supports add/sub/mul".into())),
                };
                let ao = self.op(*a);
                let bo = self.op(*b);
                let (ao, bo) =
                    self.match_operand_types(&ao, &bo, &self.ty_of(*a), &self.ty_of(*b), ty)?;
                let intr = format!("llvm.{s}{mn}.with.overflow.i{bits}");
                self.externals.insert(intr.clone());
                let pair = self.fresh();
                self.w(&format!("{pair} = call {{i{bits}, i1}} @{intr}(i{bits} {ao}, i{bits} {bo})"));
                let rv = format!("%v{dst_val}");
                self.w(&format!("{rv} = extractvalue {{i{bits}, i1}} {pair}, 0"));
                self.bind(*dst_val, ty.clone(), rv);
                let ro = format!("%v{dst_ovf}");
                self.w(&format!("{ro} = extractvalue {{i{bits}, i1}} {pair}, 1"));
                self.bind(*dst_ovf, IrType::Bool, ro);
            }
            Inst::Call { dst, func, args, ret } => {
                let callee_sym = self.resolve_callee(func)?;
                let sig = self.callee_sig(func)?;
                let dv = dst.filter(|d| *d != NO_V);
                let mut call_args: Vec<String> = Vec::new();
                if sig.sret {
                    // 呼び出し側で受け皿バッファを確保して隠し先頭引数へ渡す
                    let sz = self.prog.size_of(ret).max(1);
                    let tmp = self.fresh();
                    self.w(&format!("{tmp} = alloca [{sz} x i8], align 16"));
                    call_args.push(format!("ptr {tmp}"));
                    for (i, a) in args.iter().enumerate() {
                        let o = self.op(*a);
                        let lt = self.ty_of(*a);
                        let at = ll_type(&lt);
                        let want = sig.params.get(i).map(|s| s.as_str()).unwrap_or("ptr");
                        let adjusted = self.coerce(&o, at, want, is_signed(&lt));
                        call_args.push(format!("{want} {adjusted}"));
                    }
                    self.w(&format!("call void {callee_sym}({})", call_args.join(", ")));
                    if let Some(d) = dv {
                        self.bind(d, ret.clone(), tmp);
                    }
                } else {
                    for (i, a) in args.iter().enumerate() {
                        let o = self.op(*a);
                        let lt = self.ty_of(*a);
                        let at = ll_type(&lt);
                        let want = sig.params.get(i).map(|s| s.as_str()).unwrap_or(at);
                        let adjusted = self.coerce(&o, at, want, is_signed(&lt));
                        call_args.push(format!("{want} {adjusted}"));
                    }
                    if sig.ret == "void" {
                        self.w(&format!("call void {callee_sym}({})", call_args.join(", ")));
                    } else {
                        let r = match dv {
                            Some(d) => format!("%v{d}"),
                            None => self.fresh(),
                        };
                        self.w(&format!("{r} = call {} {callee_sym}({})", sig.ret, call_args.join(", ")));
                        if let Some(d) = dv {
                            // ランタイムの実LLVM型（例: __ngs_str_eq の i8）と
                            // 論理型（Bool=i1）の差をここで埋める
                            let want_ll = ll_type(ret);
                            let final_r = self.coerce(&r, &sig.ret, want_ll, is_signed(ret));
                            self.bind(d, ret.clone(), final_r);
                        }
                    }
                }
            }
            Inst::FieldAddr { dst, base, struct_id, field } => {
                let bo = self.op(*base);
                let off = self.prog.field_offset(*struct_id, *field);
                let r = format!("%v{dst}");
                self.w(&format!("{r} = getelementptr inbounds i8, ptr {bo}, i64 {off}"));
                self.bind(*dst, IrType::Ptr(std::rc::Rc::new(IrType::U8)), r);
            }
            Inst::ElemAddr { dst, base, index, elem } => {
                let bo = self.op(*base);
                let (io, _) = self.widen_to_i64(*index)?;
                let stride = self.prog.cell_of(elem);
                let scaled = self.fresh();
                self.w(&format!("{scaled} = mul i64 {io}, {stride}"));
                let r = format!("%v{dst}");
                self.w(&format!("{r} = getelementptr inbounds i8, ptr {bo}, i64 {scaled}"));
                self.bind(*dst, IrType::Ptr(std::rc::Rc::new(IrType::U8)), r);
            }
            Inst::AddrOff { dst, base, off } => {
                let bo = self.op(*base);
                let r = format!("%v{dst}");
                self.w(&format!("{r} = getelementptr inbounds i8, ptr {bo}, i64 {off}"));
                self.bind(*dst, IrType::Ptr(std::rc::Rc::new(IrType::U8)), r);
            }
            Inst::Cast { dst, kind, val, to } => {
                let vo = self.op(*val);
                let from = self.ty_of(*val);
                let ft = ll_type(&from);
                let tt = ll_type(to);
                let r = format!("%v{dst}");
                match kind {
                    CastKind::Trunc => self.w(&format!("{r} = trunc {ft} {vo} to {tt}")),
                    CastKind::Zext => self.w(&format!("{r} = zext {ft} {vo} to {tt}")),
                    CastKind::Sext => self.w(&format!("{r} = sext {ft} {vo} to {tt}")),
                    CastKind::Sitofp => self.w(&format!("{r} = sitofp {ft} {vo} to {tt}")),
                    CastKind::Fptosi => self.w(&format!("{r} = fptosi {ft} {vo} to {tt}")),
                    CastKind::Fptoui => self.w(&format!("{r} = fptoui {ft} {vo} to {tt}")),
                    CastKind::Fpext => self.w(&format!("{r} = fpext {ft} {vo} to {tt}")),
                    CastKind::Fptrunc => self.w(&format!("{r} = fptrunc {ft} {vo} to {tt}")),
                    CastKind::Inttoptr => self.w(&format!("{r} = inttoptr {ft} {vo} to {tt}")),
                    CastKind::Ptrtoint => self.w(&format!("{r} = ptrtoint {ft} {vo} to {tt}")),
                    CastKind::BoolToInt => self.w(&format!("{r} = zext {ft} {vo} to {tt}")),
                    CastKind::IntToBool => {
                        self.w(&format!("{r} = icmp ne {ft} {vo}, 0"));
                    }
                }
                self.bind(*dst, to.clone(), r);
            }
            Inst::Bitcast { dst, val, to } => {
                // 不透明ポインタ時代では ptr<->ptr（および値保持の int<->ptr）の
                // bitcast は同一値。重要なのは論理型の付け替え。
                let vo = self.op(*val);
                self.bind(*dst, to.clone(), vo);
            }
            Inst::CopyAgg { dst_addr, src_ptr, ty } => {
                let d = self.op(*dst_addr);
                let s = self.op(*src_ptr);
                let sz = self.prog.size_of(ty);
                self.externals.insert("llvm.memcpy.p0.p0.i64".into());
                self.w(&format!(
                    "call void @llvm.memcpy.p0.p0.i64(ptr {d}, ptr {s}, i64 {sz}, i1 false)"
                ));
            }
            Inst::RcInc { val } => {
                let o = self.op(*val);
                self.externals.insert("__ngs_rc_inc".into());
                self.w(&format!("call void @__ngs_rc_inc(ptr {o})"));
            }
            Inst::RcDec { val } => {
                let o = self.op(*val);
                self.externals.insert("__ngs_rc_dec".into());
                self.w(&format!("call void @__ngs_rc_dec(ptr {o})"));
            }
            Inst::BoxVal { dst, src, ty } => {
                let o = self.op(*src);
                let r = format!("%v{dst}");
                match ty {
                    IrType::Str => {
                        self.w(&format!("{r} = call i64 @__ngs_box_str(ptr {o})"));
                    }
                    IrType::F64 => self.w(&format!("{r} = call i64 @__ngs_box_f64(double {o})")),
                    IrType::F32 => {
                        let x = self.fresh();
                        self.w(&format!("{x} = fpext float {o} to double"));
                        self.w(&format!("{r} = call i64 @__ngs_box_f64(double {x})"));
                    }
                    IrType::Bool => {
                        let x = self.fresh();
                        self.w(&format!("{x} = zext i1 {o} to i8"));
                        self.w(&format!("{r} = call i64 @__ngs_box_bool(i8 {x})"));
                    }
                    t if t.is_int() => {
                        let x = self.fresh();
                        let bits = t.bits();
                        if is_signed(t) {
                            self.w(&format!("{x} = sext i{bits} {o} to i64"));
                        } else {
                            self.w(&format!("{x} = zext i{bits} {o} to i64"));
                        }
                        self.w(&format!("{r} = call i64 @__ngs_box_i64(i64 {x})"));
                    }
                    _ => {
                        self.w(&format!("{r} = call i64 @__ngs_box_ptr(ptr {o})"));
                    }
                }
                self.bind(*dst, IrType::I64, r);
            }
        }
        Ok(())
    }

    /// 二項演算のオペランド型を揃える（片方が Const::Int 由来の i64 の場合など）
    #[allow(clippy::too_many_arguments)]
    fn match_operand_types(
        &mut self,
        a: &str,
        b: &str,
        at: &IrType,
        bt: &IrType,
        want: &IrType,
    ) -> Result<(String, String), CodegenError> {
        let wt = ll_type(want);
        let ao = self.coerce(a, ll_type(at), wt, is_signed(at));
        let bo = self.coerce(b, ll_type(bt), wt, is_signed(bt));
        Ok((ao, bo))
    }

    /// LLVM レベルの型合わせ。必要なら変換命令を生成して新しいオペランドを返す。
    /// `signed_hint` は拡大時の sext/zext 選択に使う。
    fn coerce(&mut self, o: &str, have: &str, want: &str, signed_hint: bool) -> String {
        if have == want {
            return o.to_string();
        }
        let r = self.fresh();
        match (have, want) {
            (h, "i1") if h.starts_with('i') => {
                // 真偽値化は trunc ではなく icmp ne（下位ビットだけでは真値にならない）
                self.w(&format!("{r} = icmp ne {h} {o}, 0"));
            }
            ("i1", w) if w.starts_with('i') => {
                self.w(&format!("{r} = zext i1 {o} to {w}"));
            }
            (h, w) if h.starts_with('i') && w.starts_with('i') => {
                let hbits: u32 = h[1..].parse().unwrap_or(64);
                let wbits: u32 = w[1..].parse().unwrap_or(64);
                if wbits < hbits {
                    self.w(&format!("{r} = trunc {h} {o} to {w}"));
                } else if signed_hint {
                    self.w(&format!("{r} = sext {h} {o} to {w}"));
                } else {
                    self.w(&format!("{r} = zext {h} {o} to {w}"));
                }
            }
            ("double", "float") => self.w(&format!("{r} = fptrunc double {o} to float")),
            ("float", "double") => self.w(&format!("{r} = fpext float {o} to double")),
            ("ptr", "i64") => self.w(&format!("{r} = ptrtoint ptr {o} to i64")),
            ("i64", "ptr") => self.w(&format!("{r} = inttoptr i64 {o} to ptr")),
            _ => return o.to_string(),
        }
        r
    }

    fn resolve_callee(&mut self, name: &str) -> Result<String, CodegenError> {
        // lowering は libm の fmod を __ngs_fmod 名で呼ぶためここで向き先を付ける
        if name == "__ngs_fmod" {
            return Ok("@fmod".into());
        }
        if name.starts_with('@') {
            return Ok(name.to_string());
        }
        if let Some((from, to)) = &self.main_rename {
            if name == from {
                return Ok(ll_name(to));
            }
        }
        Ok(ll_name(name))
    }

    /// 呼び出し先の正式シグネチャを解決する（プログラム内定義 > libm > ランタイム表）
    fn callee_sig(&mut self, name: &str) -> Result<CalleeSig, CodegenError> {
        if let Some(f) = self.prog.funcs.iter().find(|f| f.name == name) {
            let sret = f.ret.is_aggregate();
            return Ok(CalleeSig {
                params: f.params.iter().map(|(_, t)| ll_type(t).to_string()).collect(),
                ret: if sret { "void".into() } else { ll_type(&f.ret).to_string() },
                sret,
            });
        }
        if name == "__ngs_fmod" {
            self.externals.insert("fmod".into());
            return Ok(CalleeSig {
                params: vec!["double".into(), "double".into()],
                ret: "double".into(),
                sret: false,
            });
        }
        if let Some((n, ps, r)) = RUNTIME_SIGS.iter().find(|(n, _, _)| **n == *name) {
            self.externals.insert((*n).to_string());
            return Ok(CalleeSig {
                params: ps.iter().map(|s| s.to_string()).collect(),
                ret: (*r).to_string(),
                sret: false,
            });
        }
        Err(CodegenError(format!("unknown callee `{name}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ngs_ir::lower::lower;

    const SRC: &str = r#"
struct Point { x: i32, y: i32 }
enum Shape { Circle(f64), Rect(i32, i32), Empty }

fn quad(a: i32) -> i32 {
    return a * a;
}

extern "C" fn puts(s: string);

export "C" fn answer() -> i32 {
    val p = Point { x: 3, y: 4 };
    var total = p.x + p.y;
    val arr = [1, 2, 3];
    total += arr[2];
    val l: List<i32> = List.new();
    l.push(10);
    l.push(20);
    total = total + l.get(1);
    val r = Rc.new(9);
    val s: Shape = Shape.Rect(2, 5);
    val area = match s {
        Circle(rad) => rad * 3.0,
        Rect(w, h) => (w * h) as f64,
        _ => 0.0,
    };
    if area > 5.0 && total < 100 {
        print("area big");
    } else {
        print(total);
    }
    var i = 0;
    while true {
        i = i + 1;
        if i == 2 { break; }
    }
    for k in 0..quad(4) {
        if k == 1 { continue; }
        total = total + k;
    }
    val name = "core";
    if name == "core" { total = total + 1; }
    val m = match total { 40 => 1, _ => 0 };
    return total + m - 45;
}
"#;

    #[test]
    fn smoke_codegen_llvm() {
        let file = ngs_parser::parse_source(SRC, "test.ngs").expect("parse");
        let typed = ngs_sema::check(&file).expect("sema");
        let ir = lower(&typed).expect("lower");
        let opts = LlvmOptions { gen_main_wrapper: false, ..Default::default() };
        let text = generate(&ir, &opts).expect("codegen");
        assert!(text.contains("define"), "should contain function definitions");
        println!("{text}");
    }

    #[test]
    fn main_wrapper_emitted() {
        let src = r#"
fn main() -> i32 {
    val p = Point { x: 3, y: 4 };
    return p.x + p.y;
}
struct Point { x: i32, y: i32 }
"#;
        let file = ngs_parser::parse_source(src, "t.ngs").unwrap();
        let typed = ngs_sema::check(&file).unwrap();
        let ir = lower(&typed).unwrap();
        let text = generate(&ir, &LlvmOptions::default()).expect("codegen");
        assert!(text.contains("define i32 @main()"), "main wrapper should exist");
        assert!(text.contains("store ptr"), "aggregate return should use sret");
    }

    #[test]
    fn rejects_mainless_wrapper() {
        let src = "fn f() -> i32 { return 1; }";
        let file = ngs_parser::parse_source(src, "t.ngs").unwrap();
        let typed = ngs_sema::check(&file).unwrap();
        let ir = lower(&typed).unwrap();
        let err = generate(&ir, &LlvmOptions::default()).unwrap_err();
        assert!(err.0.contains("main"));
    }
}
