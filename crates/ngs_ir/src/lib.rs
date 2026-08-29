//! NGS-IR: NagiScript独自の中間表現（簡易SSA形式）。
//! SemaのTypedProgramをここで lowering し、ngs_codegen_llvm が LLVM IR へ変換する。
//! この段階ではLLVMに依存しない（仕様2節アーキテクチャ）。

use std::rc::Rc;

pub type V = usize; // 仮想レジスタID

#[derive(Debug, Clone, PartialEq)]
pub enum IrType {
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
    Ptr(Rc<IrType>),
    Array(Rc<IrType>, u64),
    Struct(usize),
    Enum(usize),
}

impl IrType {
    pub fn is_aggregate(&self) -> bool {
        matches!(self, IrType::Struct(_) | IrType::Enum(_) | IrType::Array(..))
    }
    pub fn is_float(&self) -> bool {
        matches!(self, IrType::F32 | IrType::F64)
    }
    pub fn is_int(&self) -> bool {
        matches!(
            self,
            IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::U8 | IrType::U16
                | IrType::U32 | IrType::U64 | IrType::Usize | IrType::Isize | IrType::Bool
        )
    }
    /// ビット幅（スカラー）
    pub fn bits(&self) -> u32 {
        match self {
            IrType::Bool | IrType::I8 | IrType::U8 => 8,
            IrType::I16 | IrType::U16 => 16,
            IrType::I32 | IrType::U32 | IrType::F32 => 32,
            _ => 64,
        }
    }
    /// バイトサイズ（概算・アライン8想定）
    pub fn size(&self) -> u64 {
        match self {
            IrType::Void => 0,
            IrType::Str => 16,
            IrType::F32 => 4,
            IrType::F64 => 8,
            IrType::Ptr(_) => 8,
            IrType::Array(t, n) => t.size() * n,
            IrType::Struct(_) => 0, // program参照が必要なためlowering後は使わない
            IrType::Enum(_) => 24, // tag(8) + payload(8x2)
            _ => (self.bits() as u64).div_ceil(8),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrBin {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrPred {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    Trunc,
    Zext,
    Sext,
    Sitofp,
    Fptosi,
    Fptoui,
    Fpext,
    Fptrunc,
    Inttoptr,
    Ptrtoint,
    BoolToInt,
    IntToBool,
}

#[derive(Debug, Clone)]
pub enum Const {
    Int(u64),
    Float(f64),
    Bool(bool),
    Str(u32), // 文字列リテラルid
    NullPtr,
}

#[derive(Debug, Clone)]
pub enum Inst {
    Const { dst: V, val: Const },
    Alloca { dst: V, ty: IrType },
    Load { dst: V, addr: V, ty: IrType },
    Store { addr: V, val: V, ty: IrType },
    BinOp { dst: V, op: IrBin, ty: IrType, a: V, b: V },
    CmpOp { dst: V, pred: IrPred, ty: IrType, a: V, b: V },
    /// オーバーフロー検出付き演算（llvm.*.with.overflow への変換をcodegenで行う）
    CheckedBin { dst_val: V, dst_ovf: V, op: IrBin, ty: IrType, a: V, b: V },
    Call { dst: Option<V>, func: String, args: Vec<V>, ret: IrType },
    FieldAddr { dst: V, base: V, struct_id: usize, field: usize },
    ElemAddr { dst: V, base: V, index: V, elem: IrType },
    /// base + off バイト（enumペイロード・Strセル等の生オフセットアクセス）
    AddrOff { dst: V, base: V, off: u64 },
    Cast { dst: V, kind: CastKind, val: V, to: IrType },
    Bitcast { dst: V, val: V, to: IrType },
    /// 集約値のコピー（memcpy）
    CopyAgg { dst_addr: V, src_ptr: V, ty: IrType },
    RcInc { val: V },
    RcDec { val: V },
    /// JSX用の値ボックス化 → CrValue{tag:i32,bits:u64}
    BoxVal { dst: V, src: V, ty: IrType },
}

#[derive(Debug, Clone)]
pub enum Term {
    Ret(Option<V>),
    Br(String),
    CondBr(V, String, String),
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct IrBlock {
    pub label: String,
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<(String, IrType)>,
    pub ret: IrType,
    pub blocks: Vec<IrBlock>,
    pub is_decl: bool,
    pub cconv_c: bool,
}

impl IrFunction {
    pub fn find_block(&self, label: &str) -> Option<usize> {
        self.blocks.iter().position(|b| b.label == label)
    }
}

#[derive(Debug, Clone)]
pub struct IrStruct {
    pub mangled: String,
    pub fields: Vec<(String, IrType)>,
    pub is_list: bool,
}

#[derive(Debug, Clone)]
pub struct IrEnum {
    pub mangled: String,
    pub variants: Vec<Vec<IrType>>,
}

#[derive(Debug, Clone)]
pub struct GlobalString {
    pub content: String,
}

#[derive(Debug, Default)]
pub struct IrProgram {
    pub structs: Vec<IrStruct>,
    pub enums: Vec<IrEnum>,
    pub funcs: Vec<IrFunction>,
    pub strings: Vec<GlobalString>,
    pub main_name: Option<String>,
    pub exports: Vec<(String, Vec<(String, IrType)>, IrType)>,
}

fn align8(n: u64) -> u64 {
    n.div_ceil(8) * 8
}

impl IrProgram {
    /// 値が「実体へのポインタ」で表される型か
    pub fn is_val_ptr(t: &IrType) -> bool {
        t.is_aggregate() || matches!(t, IrType::Str)
    }

    /// 値の実体サイズ（バイト）。構造体は round_up(field,8) の累積。
    pub fn size_of(&self, t: &IrType) -> u64 {
        match t {
            IrType::Void => 0,
            IrType::Bool | IrType::I8 | IrType::U8 => 1,
            IrType::I16 | IrType::U16 => 2,
            IrType::F32 | IrType::I32 | IrType::U32 => 4,
            IrType::I64 | IrType::U64 | IrType::Usize | IrType::Isize | IrType::F64 | IrType::Ptr(_) => 8,
            IrType::Str => 16,
            IrType::Array(el, n) => self.cell_of(el) * n,
            IrType::Struct(sid) => self
                .structs
                .get(*sid)
                .map(|s| {
                    s.fields.iter().map(|(_, ft)| align8(self.size_of(ft).max(1))).sum::<u64>().max(1)
                })
                .unwrap_or(8),
            IrType::Enum(_) => 24,
        }
    }

    /// ストレージセルのサイズ（集約・Strはポインタ1個分）
    pub fn cell_of(&self, t: &IrType) -> u64 {
        if Self::is_val_ptr(t) {
            8
        } else {
            self.size_of(t).max(1)
        }
    }

    /// 構造体 sid のフィールド idx のオフセット
    pub fn field_offset(&self, sid: usize, idx: usize) -> u64 {
        let mut off = 0u64;
        if let Some(s) = self.structs.get(sid) {
            for (_, t) in s.fields.iter().take(idx) {
                off += align8(self.size_of(t).max(1));
            }
        }
        off
    }
}

// ---------------------------------------------------------------------------
// Builder — lowering 用の便利ラッパ
// ---------------------------------------------------------------------------

pub struct FnBuilder {
    pub func: IrFunction,
    pub cur: usize,
    next_v: usize,
    block_counter: usize,
    pub strings: Vec<String>, // 新規に必要になったリテラル（program.stringsへ反映）
    /// プログラム全体のリテラルプールにおける本関数分の開始オフセット。
    /// Const::Str(id) の id は「グローバルプールのインデックス」として解釈されるため、
    /// lowering 側で事前に設定する（単一関数の手組みテストでは既定 0 のままでよい）。
    strings_base: u32,
}

pub const NO_V: V = usize::MAX;

impl FnBuilder {
    pub fn new(name: String, params: Vec<(String, IrType)>, ret: IrType, cconv_c: bool) -> Self {
        // パラメータ仮想レジスタは 0..params.len()-1 を占有する（lowering 規約）。
        // ローカルの割当はその後ろから始める。
        let next_v = params.len();
        FnBuilder {
            func: IrFunction {
                name,
                params,
                ret,
                blocks: vec![IrBlock { label: "entry".into(), insts: vec![], term: Term::Unreachable }],
                is_decl: false,
                cconv_c,
            },
            cur: 0,
            next_v,
            block_counter: 0,
            strings: vec![],
            strings_base: 0,
        }
    }

    /// グローバル文字列プール内での本関数の開始オフセットを設定する
    pub fn set_strings_base(&mut self, base: u32) {
        self.strings_base = base;
    }

    pub fn v(&mut self) -> V {
        let v = self.next_v;
        self.next_v += 1;
        v
    }

    pub fn push(&mut self, i: Inst) -> V {
        let dst = match &i {
            Inst::Const { dst, .. }
            | Inst::Alloca { dst, .. }
            | Inst::Load { dst, .. }
            | Inst::BinOp { dst, .. }
            | Inst::CmpOp { dst, .. }
            | Inst::CheckedBin { dst_val: dst, .. }
            | Inst::Call { dst: Some(dst), .. }
            | Inst::FieldAddr { dst, .. }
            | Inst::ElemAddr { dst, .. }
            | Inst::AddrOff { dst, .. }
            | Inst::Cast { dst, .. }
            | Inst::Bitcast { dst, .. }
            | Inst::BoxVal { dst, .. } => *dst,
            _ => NO_V,
        };
        self.func.blocks[self.cur].insts.push(i);
        dst
    }

    fn newv(&mut self, mut i: Inst) -> V {
        let dst = self.v();
        if let Inst::Const { dst: ref mut d, .. } = i {
            *d = dst;
        }
        self.func.blocks[self.cur].insts.push(i);
        dst
    }

    // --- 命令ヘルパ ---
    pub fn const_int(&mut self, v: u64) -> V {
        self.newv(Inst::Const { dst: 0, val: Const::Int(v) })
    }
    pub fn const_float(&mut self, v: f64) -> V {
        self.newv(Inst::Const { dst: 0, val: Const::Float(v) })
    }
    pub fn const_bool(&mut self, v: bool) -> V {
        self.newv(Inst::Const { dst: 0, val: Const::Bool(v) })
    }
    pub fn const_null(&mut self) -> V {
        self.newv(Inst::Const { dst: 0, val: Const::NullPtr })
    }
    pub fn str_lit(&mut self, s: &str) -> V {
        let id = self.strings_base + self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.newv(Inst::Const { dst: 0, val: Const::Str(id) })
    }
    pub fn alloca(&mut self, ty: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::Alloca { dst, ty });
        dst
    }
    pub fn load(&mut self, addr: V, ty: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::Load { dst, addr, ty });
        dst
    }
    pub fn store(&mut self, addr: V, val: V, ty: IrType) {
        self.func.blocks[self.cur].insts.push(Inst::Store { addr, val, ty });
    }
    pub fn binop(&mut self, op: IrBin, ty: IrType, a: V, b: V) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::BinOp { dst, op, ty, a, b });
        dst
    }
    pub fn cmp(&mut self, pred: IrPred, ty: IrType, a: V, b: V) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::CmpOp { dst, pred, ty, a, b });
        dst
    }
    pub fn checked_bin(&mut self, op: IrBin, ty: IrType, a: V, b: V) -> (V, V) {
        let dv = self.v();
        let dovf = self.v();
        self.func.blocks[self.cur].insts.push(Inst::CheckedBin {
            dst_val: dv,
            dst_ovf: dovf,
            op,
            ty,
            a,
            b,
        });
        (dv, dovf)
    }
    pub fn call(&mut self, func: &str, args: Vec<V>, ret: IrType) -> Option<V> {
        if ret == IrType::Void {
            self.func.blocks[self.cur].insts.push(Inst::Call { dst: None, func: func.into(), args, ret });
            None
        } else {
            let dst = self.v();
            self.func.blocks[self.cur].insts.push(Inst::Call { dst: Some(dst), func: func.into(), args, ret: ret.clone() });
            Some(dst)
        }
    }
    pub fn field_addr(&mut self, base: V, struct_id: usize, field: usize) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::FieldAddr { dst, base, struct_id, field });
        dst
    }
    pub fn elem_addr(&mut self, base: V, index: V, elem: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::ElemAddr { dst, base, index, elem });
        dst
    }
    pub fn addr_off(&mut self, base: V, off: u64) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::AddrOff { dst, base, off });
        dst
    }
    pub fn cast(&mut self, kind: CastKind, val: V, to: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::Cast { dst, kind, val, to });
        dst
    }
    pub fn bitcast(&mut self, val: V, to: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::Bitcast { dst, val, to });
        dst
    }
    pub fn copy_agg(&mut self, dst_addr: V, src_ptr: V, ty: IrType) {
        self.func.blocks[self.cur].insts.push(Inst::CopyAgg { dst_addr, src_ptr, ty });
    }
    pub fn rc_inc(&mut self, val: V) {
        self.func.blocks[self.cur].insts.push(Inst::RcInc { val });
    }
    pub fn rc_dec(&mut self, val: V) {
        self.func.blocks[self.cur].insts.push(Inst::RcDec { val });
    }
    pub fn box_val(&mut self, src: V, ty: IrType) -> V {
        let dst = self.v();
        self.func.blocks[self.cur].insts.push(Inst::BoxVal { dst, src, ty });
        dst
    }

    // --- 制御フロー ---
    pub fn new_block(&mut self, hint: &str) -> (usize, String) {
        let id = self.block_counter;
        self.block_counter += 1;
        let label = format!("{hint}{id}");
        self.func.blocks.push(IrBlock { label: label.clone(), insts: vec![], term: Term::Unreachable });
        (self.func.blocks.len() - 1, label)
    }
    pub fn position(&mut self, idx: usize) {
        self.cur = idx;
    }
    pub fn set_term(&mut self, t: Term) {
        self.func.blocks[self.cur].term = t;
    }
    pub fn br(&mut self, label: &str) {
        self.set_term(Term::Br(label.to_string()));
    }
    pub fn cond_br(&mut self, cond: V, t: &str, f: &str) {
        self.set_term(Term::CondBr(cond, t.to_string(), f.to_string()));
    }
    pub fn ret(&mut self, v: Option<V>) {
        self.set_term(Term::Ret(v));
    }
    pub fn unreachable(&mut self) {
        self.set_term(Term::Unreachable);
    }
}

/// lowering エントリポイント
pub mod lower;

/// IR のテキストダンプ（デバッグ用）
pub mod dump;
