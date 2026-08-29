//! TypedProgram -> IrProgram lowering。
//!
//! ## 表現規約（codegen もこれに従う）
//! - struct/enum/array/str の「値」はメモリ上の実体へのポインタで表現する
//!   (is_val_ptr == true)。ローカル変数・フィールド・配列要素・enumペイロードの
//!   ストレージは、そのポインタを保持する 8バイトの「セル」である。
//! - Str の実体は {data: *u8, len: usize} の16バイトセル。文字列は不変であり
//!   リテラル等の共有は安全。値 = セルへのポインタ。
//! - Rc<T> の値はオブジェクト先頭ポインタ。レイアウト: {count: u64 @0, data: T @8}。
//! - enum のレイアウト: {tag: u64 @0, payload0 @8, payload1 @16}。各ペイロードスロット
//!   は8バイトで、val-ptr 型ならポインタが、スカラー型ならそのままの値が入る。
//!   ペイロードは最大2個まで（それ以上はコンパイルエラー）。
//! - struct のフィールドオフセットは round_up(size(field), 8) の累積。
//! - ローカルはすべて alloca セル + load/store。パラメータも entry でセルへ退避
//!   （パラメータ仮想レジスタは 0..n-1）。
//! - 集約の束縛・代入は原義コピー（copy_agg）。ただし新規生成値
//!   (Call / *Lit / VariantCtor / Props) と Str はそのまま共有する。
//! - Rc の束縛・代入は rc_inc / 古い値の rc_dec。Call 結果の直接移転は inc 不要。
//! - 安全な文脈の整数演算(e.checked)は with.overflow へ、List 添字は境界検査を挟む。

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::*;
use ngs_ast::{BinOp, UnOp};
use ngs_sema::{Callee, Intrinsic, MonoFn, TArm, TBlock, TExpr, TExprKind, TFStringPart, TPattern, TStmt, Ty, TypedProgram};

// ---------------------------------------------------------------------------
// 型変換
// ---------------------------------------------------------------------------

pub fn conv_ty(t: &Ty) -> IrType {
    match t {
        Ty::Void => IrType::Void,
        Ty::Bool => IrType::Bool,
        Ty::Str => IrType::Str,
        Ty::I8 => IrType::I8,
        Ty::I16 => IrType::I16,
        Ty::I32 => IrType::I32,
        Ty::I64 => IrType::I64,
        Ty::U8 => IrType::U8,
        Ty::U16 => IrType::U16,
        Ty::U32 => IrType::U32,
        Ty::U64 => IrType::U64,
        Ty::Usize => IrType::Usize,
        Ty::Isize => IrType::Isize,
        Ty::F32 => IrType::F32,
        Ty::F64 => IrType::F64,
        Ty::Ptr(inner) => IrType::Ptr(Rc::new(conv_ty(inner))),
        Ty::Array(inner, n) => IrType::Array(Rc::new(conv_ty(inner)), *n),
        Ty::Struct(..) | Ty::Enum(..) => {
            // mono index は Lowerer が解決するため仮で Struct(0)。実体は lower 内で置換。
            IrType::Void
        }
        Ty::Generic(_) => IrType::Void,
        Ty::RcT(_) | Ty::Props => IrType::Ptr(Rc::new(IrType::U8)),
    }
}

/// 値が「実体へのポインタ」で表される型か
pub fn is_val_ptr(t: &IrType) -> bool {
    t.is_aggregate() || matches!(t, IrType::Str)
}

/// セルに入る型（val-ptr 型ならポインタ）
fn cell_ty(t: &IrType) -> IrType {
    if is_val_ptr(t) {
        IrType::Ptr(Rc::new(t.clone()))
    } else {
        t.clone()
    }
}

fn is_rc_ir(t: &IrType) -> bool {
    matches!(t, IrType::Ptr(inner) if matches!(**inner, IrType::U8))
}

fn is_rc_ty(t: &Ty) -> bool {
    matches!(t, Ty::RcT(_))
}

fn pointee(t: &IrType) -> Option<IrType> {
    match t {
        IrType::Ptr(inner) => Some((**inner).clone()),
        _ => None,
    }
}

/// 新規生成値か（束縛時にコピー不要）
fn is_fresh(e: &TExpr) -> bool {
    matches!(
        e.kind,
        TExprKind::Call(..)
            | TExprKind::StructLit { .. }
            | TExprKind::ArrayLit(..)
            | TExprKind::VariantCtor { .. }
            | TExprKind::Props(..)
            | TExprKind::UninitPlaceholder
    )
}

// ---------------------------------------------------------------------------
// コンテキスト
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LocalSlot {
    /// ストレージセルのアドレス（常時メモリ）
    cell: V,
    /// 論理型
    ty: IrType,
    /// Rc 管理対象か
    rc: bool,
    /// List<T> ローカル（脱出時 __ngs_list_free）
    list_elem_size: Option<u64>,
}

struct FnCtx {
    b: FnBuilder,
    locals: HashMap<String, LocalSlot>,
    order: Vec<String>,
    breaks: Vec<String>,
    continues: Vec<String>,
}

impl FnCtx {
    fn declare(&mut self, name: String, slot: LocalSlot) {
        self.order.push(name.clone());
        self.locals.insert(name, slot);
    }
    fn truncate(&mut self, base: usize) {
        while self.order.len() > base {
            let n = self.order.pop().unwrap();
            self.locals.remove(&n);
        }
    }
}

struct Lowerer<'a> {
    prog: &'a TypedProgram,
    out: IrProgram,
    /// 現在 lowering 中の関数の戻り型（return 文の値変換に使用）
    cur_ret: IrType,
}

pub fn lower(prog: &TypedProgram) -> Result<IrProgram, String> {
    let mut lp = Lowerer { prog, out: IrProgram::default(), cur_ret: IrType::Void };
    for ms in &prog.structs {
        lp.out.structs.push(IrStruct {
            mangled: ms.mangled.clone(),
            fields: ms
                .fields
                .iter()
                .map(|(n, t)| lp.conv(t).map(|ir| (n.clone(), ir)))
                .collect::<Result<Vec<_>, String>>()?,
            is_list: ms.is_list,
        });
    }
    for me in &prog.enums {
        for (_, pts) in &me.variants {
            if pts.len() > 2 {
                return Err(format!(
                    "enum `{}`: more than 2 payload values is not supported",
                    me.mangled
                ));
            }
        }
        lp.out.enums.push(IrEnum {
            mangled: me.mangled.clone(),
            variants: me
                .variants
                .iter()
                .map(|(_, ts)| ts.iter().map(|t| lp.conv(t)).collect::<Result<Vec<_>, String>>())
                .collect::<Result<Vec<_>, String>>()?,
        });
    }
    for mf in &prog.funcs {
        lp.lower_fn(mf)?;
    }
    lp.out.main_name = prog.funcs.iter().find(|f| f.is_user_main).map(|f| f.mangled.clone());
    for e in &prog.exports {
        lp.out.exports.push((
            e.name.clone(),
            e.params
                .iter()
                .map(|(n, t)| lp.conv(t).map(|ir| (n.clone(), ir)))
                .collect::<Result<Vec<_>, String>>()?,
            lp.conv(&e.ret)?,
        ));
    }
    Ok(lp.out)
}

impl<'a> Lowerer<'a> {
    /// mono index 解決込みの型変換
    fn conv(&self, t: &Ty) -> Result<IrType, String> {
        match t {
            Ty::Struct(id, subs) => {
                let idx = self
                    .prog
                    .structs
                    .iter()
                    .position(|ms| ms.def_id == *id && &ms.substs == subs)
                    .ok_or_else(|| format!("uninterned struct instance {}", t.display()))?;
                Ok(IrType::Struct(idx))
            }
            Ty::Enum(id, subs) => {
                let idx = self
                    .prog
                    .enums
                    .iter()
                    .position(|me| me.def_id == *id && &me.substs == subs)
                    .ok_or_else(|| format!("uninterned enum instance {}", t.display()))?;
                Ok(IrType::Enum(idx))
            }
            other => Ok(conv_ty(other)),
        }
    }

    fn struct_by_mangled(&self, mangled: &str) -> Result<usize, String> {
        self.out.structs.iter().position(|s| s.mangled == mangled).ok_or_else(|| format!("unknown struct `{mangled}`"))
    }
    fn enum_by_mangled(&self, mangled: &str) -> Result<usize, String> {
        self.out.enums.iter().position(|s| s.mangled == mangled).ok_or_else(|| format!("unknown enum `{mangled}`"))
    }
    fn mono_struct_of(&self, t: &Ty) -> Result<usize, String> {
        match t {
            Ty::Struct(id, subs) => self
                .prog
                .structs
                .iter()
                .position(|ms| ms.def_id == *id && &ms.substs == subs)
                .ok_or_else(|| format!("uninterned struct instance {}", t.display())),
            _ => Err(format!("not a struct type: {}", t.display())),
        }
    }
    /// enum ペイロード i のバイトオフセット（tag の後）
    fn payload_off(i: usize) -> u64 {
        8 + 8 * i as u64
    }

    // ------------------------------------------------------------------
    // 関数
    // ------------------------------------------------------------------

    fn lower_fn(&mut self, mf: &MonoFn) -> Result<(), String> {
        let params_ir: Vec<(String, IrType)> =
            mf.params.iter().map(|(n, t)| Ok((n.clone(), self.conv(t)?))).collect::<Result<_, String>>()?;
        let ret = self.conv(&mf.ret)?;
        self.cur_ret = ret.clone();

        if mf.body.is_none() || mf.extern_abi.is_some() {
            self.out.funcs.push(IrFunction {
                name: mf.mangled.clone(),
                params: params_ir,
                ret,
                blocks: vec![],
                is_decl: true,
                cconv_c: mf.extern_abi.is_some() || mf.export_abi,
            });
            return Ok(());
        }

        let mut ctx = FnCtx {
            b: FnBuilder::new(mf.mangled.clone(), params_ir.clone(), ret.clone(), mf.export_abi),
            locals: HashMap::new(),
            order: vec![],
            breaks: vec![],
            continues: vec![],
        };
        // Const::Str の id はグローバル文字列プールのインデックスとして解釈されるため、
        // 本関数の開始オフセットを事前に設定する
        ctx.b.set_strings_base(self.out.strings.len() as u32);

        for (i, (n, t)) in params_ir.iter().enumerate() {
            let ct = cell_ty(t);
            let cell = ctx.b.alloca(ct.clone());
            ctx.b.store(cell, i, ct);
            ctx.declare(
                n.clone(),
                LocalSlot { cell, ty: t.clone(), rc: is_rc_ir(t), list_elem_size: None },
            );
        }

        let body = mf.body.as_ref().unwrap().clone();
        let val = self.run_block(&mut ctx, &body)?;

        if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
            if mf.ret == Ty::Void {
                ctx.b.ret(None);
            } else if let Some(v) = val {
                ctx.b.ret(Some(v));
            } else {
                return Err(format!(
                    "function `{}` can reach the end of its body without returning `{}`",
                    mf.mangled,
                    mf.ret.display()
                ));
            }
        }
        // 文字列リテラルの反映
        let strs = std::mem::take(&mut ctx.b.strings);
        self.out.strings.extend(strs.into_iter().map(|content| GlobalString { content }));
        self.out.funcs.push(ctx.b.func);
        Ok(())
    }


    // ------------------------------------------------------------------
    // ブロック / 後片付け
    // ------------------------------------------------------------------

    /// スコープ base 以降のローカルを逆順に解放し、マップから除去する
    fn cleanup_locals(&mut self, ctx: &mut FnCtx, base: usize) {
        while ctx.order.len() > base {
            let name = ctx.order.pop().unwrap();
            let slot = ctx.locals.remove(&name).unwrap();
            if slot.rc {
                let ct = cell_ty(&slot.ty);
                let v = ctx.b.load(slot.cell, ct);
                ctx.b.rc_dec(v);
            } else if let Some(esz) = slot.list_elem_size {
                let ct = cell_ty(&slot.ty);
                let lp = ctx.b.load(slot.cell, ct);
                ctx.b.call("__ngs_list_free", vec![lp], IrType::Void);
                let _ = esz;
            }
        }
    }

    /// ブロック本体の実行。tail があればその値を返す（スコープ内ローカルは解放済み）。
    fn run_block(&mut self, ctx: &mut FnCtx, blk: &TBlock) -> Result<Option<V>, String> {
        let base = ctx.order.len();
        for s in &blk.stmts {
            if !matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                break;
            }
            self.stmt(ctx, s)?;
        }
        let mut val = None;
        if blk.tail.is_some() && matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
            let tail = blk.tail.as_ref().expect("tail checked above");
            val = Some(self.materialize_tail(ctx, tail)?);
        }
        self.cleanup_locals(ctx, base);
        Ok(val)
    }

    /// ブロック tail の値を、スコープ解放後も有効な形に調整する。
    /// - Rc ローカルをそのまま返す場合は参照数を増やす
    /// - 集約ローカルをそのまま返す場合は新バッファへコピー
    fn materialize_tail(&mut self, ctx: &mut FnCtx, e: &TExpr) -> Result<V, String> {
        let ty = self.conv(&e.ty)?;
        if is_rc_ir(&ty) {
            if let TExprKind::Local(name) = &e.kind {
                if let Some(slot) = ctx.locals.get(name) {
                    let ct = cell_ty(&slot.ty);
                    let v = ctx.b.load(slot.cell, ct);
                    ctx.b.rc_inc(v);
                    return Ok(v);
                }
            }
            return self.expr(ctx, e);
        }
        let v = self.expr(ctx, e)?;
        if is_val_ptr(&ty) && matches!(e.kind, TExprKind::Local(_)) && !matches!(ty, IrType::Str) {
            let tmp = ctx.b.alloca(ty.clone());
            ctx.b.copy_agg(tmp, v, ty);
            return Ok(tmp);
        }
        Ok(v)
    }

    // ------------------------------------------------------------------
    // 文
    // ------------------------------------------------------------------

    fn stmt(&mut self, ctx: &mut FnCtx, s: &TStmt) -> Result<(), String> {
        match s {
            TStmt::Let(name, ty, init, _) => {
                let ity = self.conv(ty)?;
                let v = self.bind_value(ctx, init)?;
                let ct = cell_ty(&ity);
                let cell = ctx.b.alloca(ct.clone());
                let rc = is_rc_ty(ty);
                let transferred = matches!(init.kind, TExprKind::Call(..));
                self.write_cell(ctx, cell, v, &ct, rc, transferred, true);
                let list_elem_size = 
                    matches!(ity, IrType::Struct(si) if self.out.structs[si].is_list).then_some(8u64);
                ctx.declare(
                    name.clone(),
                    LocalSlot { cell, ty: ity, rc: is_rc_ty(ty), list_elem_size },
                );
            }
            TStmt::Assign(target, op, value) => {
                let taddr = self.lvalue_addr(ctx, target)?;
                let tty = self.conv(&target.ty)?;
                let ct = cell_ty(&tty);
                match op {
                    None => self.write_cell_expr(ctx, taddr, value, &ct, false),
                    Some(o) => {
                        let old = self.read_cell(ctx, taddr, &tty)?;
                        let rhs = self.expr(ctx, value)?;
                        let res = self.arith_binop(ctx, *o, tty.clone(), old, rhs, value.checked || target.checked)?;
                        ctx.b.store(taddr, res, ct);
                    }
                }
            }
            TStmt::Expr(e) => {
                self.expr(ctx, e)?;
            }
            TStmt::Return(v) => match v {
                Some(e) => {
                    let val = self.expr(ctx, e)?;
                    let to = self.cur_ret.clone();
                    let val = self.cast_value(ctx, e, val, to)?;
                    ctx.b.ret(Some(val));
                }
                None => ctx.b.ret(None),
            },
            TStmt::While(cond, body) => {
                let (cb, cl) = ctx.b.new_block("while.cond");
                let (bb, bl) = ctx.b.new_block("while.body");
                let (eb, el) = ctx.b.new_block("while.end");
                ctx.b.br(&cl);
                ctx.b.position(cb);
                let cv = self.expr(ctx, cond)?;
                ctx.b.cond_br(cv, &bl, &el);
                ctx.b.position(bb);
                ctx.breaks.push(el.clone());
                ctx.continues.push(cl.clone());
                self.run_block_void(ctx, body)?;
                ctx.breaks.pop();
                ctx.continues.pop();
                if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                    ctx.b.br(&cl);
                }
                ctx.b.position(eb);
            }
            TStmt::ForRange(var, vty, start, end, body) => {
                let ity = self.conv(vty)?;
                let sv = self.expr(ctx, start)?;
                let ev = self.expr(ctx, end)?;
                let ia = ctx.b.alloca(ity.clone());
                ctx.b.store(ia, sv, ity.clone());
                let ea = ctx.b.alloca(ity.clone());
                ctx.b.store(ea, ev, ity.clone());

                let (cb, cl) = ctx.b.new_block("for.cond");
                let (bb, bl) = ctx.b.new_block("for.body");
                let (sb, sl) = ctx.b.new_block("for.step");
                let (eb, el) = ctx.b.new_block("for.end");
                ctx.b.br(&cl);
                ctx.b.position(cb);
                let iv = ctx.b.load(ia, ity.clone());
                let ev2 = ctx.b.load(ea, ity.clone());
                let lt = ctx.b.cmp(IrPred::Lt, ity.clone(), iv, ev2);
                ctx.b.cond_br(lt, &bl, &el);

                ctx.b.position(bb);
                ctx.declare(
                    var.clone(),
                    LocalSlot { cell: ia, ty: ity.clone(), rc: false, list_elem_size: None },
                );
                ctx.breaks.push(el.clone());
                ctx.continues.push(sl.clone());
                let base = ctx.order.len();
                for st in &body.stmts {
                    if !matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                        break;
                    }
                    self.stmt(ctx, st)?;
                }
                ctx.breaks.pop();
                ctx.continues.pop();
                ctx.truncate(base);
                if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                    ctx.b.br(&sl);
                }
                ctx.b.position(sb);
                let iv2 = ctx.b.load(ia, ity.clone());
                let one = ctx.b.const_int(1);
                let inc = ctx.b.binop(IrBin::Add, ity.clone(), iv2, one);
                ctx.b.store(ia, inc, ity.clone());
                ctx.b.br(&cl);
                ctx.b.position(eb);
            }
            TStmt::ForC(init, cond, step, body) => {
                let pre_base = ctx.order.len();
                if let Some(i) = init {
                    self.stmt(ctx, i)?;
                }
                let (cb, cl) = ctx.b.new_block("cfor.cond");
                let (bb, bl) = ctx.b.new_block("cfor.body");
                let (sb, sl) = ctx.b.new_block("cfor.step");
                let (eb, el) = ctx.b.new_block("cfor.end");
                ctx.b.br(&cl);
                ctx.b.position(cb);
                if let Some(c) = cond {
                    let cv = self.expr(ctx, c)?;
                    ctx.b.cond_br(cv, &bl, &el);
                } else {
                    ctx.b.br(&bl);
                }
                ctx.b.position(bb);
                ctx.breaks.push(el.clone());
                ctx.continues.push(sl.clone());
                let base = ctx.order.len();
                self.run_block_stmts(ctx, body, base)?;
                ctx.breaks.pop();
                ctx.continues.pop();
                if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                    ctx.b.br(&sl);
                }
                ctx.b.position(sb);
                if let Some(st) = step {
                    self.stmt(ctx, st)?;
                }
                ctx.b.br(&cl);
                ctx.b.position(eb);
                ctx.truncate(pre_base);
            }
            TStmt::Break => {
                let t = ctx.breaks.last().cloned().ok_or("`break` outside loop")?;
                ctx.b.br(&t);
                let (db, _) = ctx.b.new_block("after.br");
                ctx.b.position(db);
            }
            TStmt::Continue => {
                let t = ctx.continues.last().cloned().ok_or("`continue` outside loop")?;
                ctx.b.br(&t);
                let (db, _) = ctx.b.new_block("after.cont");
                ctx.b.position(db);
            }
        }
        Ok(())
    }

    /// ループ本体など、値を捨てるブロック実行
    fn run_block_void(&mut self, ctx: &mut FnCtx, blk: &TBlock) -> Result<(), String> {
        let base = ctx.order.len();
        self.run_block_stmts(ctx, blk, base)?;
        Ok(())
    }

    fn run_block_stmts(&mut self, ctx: &mut FnCtx, blk: &TBlock, base: usize) -> Result<(), String> {
        for s in &blk.stmts {
            if !matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                break;
            }
            self.stmt(ctx, s)?;
        }
        // tail は値が必要でないため評価しない（副作用のみの式は stmt 化済みのはず）
        self.cleanup_locals(ctx, base);
        Ok(())
    }

    // ------------------------------------------------------------------
    // 式
    // ------------------------------------------------------------------

    fn expr(&mut self, ctx: &mut FnCtx, e: &TExpr) -> Result<V, String> {
        let ty = self.conv(&e.ty)?;
        if matches!(ty, IrType::Void) && !matches!(e.kind, TExprKind::Call(..) | TExprKind::If { .. } | TExprKind::Block(_) | TExprKind::Match { .. } | TExprKind::Props(..)) {
            return Err(format!("expression of type void used as a value ({:?})", e.kind));
        }
        let v = self.expr_inner(ctx, e, &ty)?;
        Ok(v)
    }

    fn expr_inner(&mut self, ctx: &mut FnCtx, e: &TExpr, ty: &IrType) -> Result<V, String> {
        match &e.kind {
            TExprKind::Int(vv) => match ty {
                IrType::F32 | IrType::F64 => Ok(ctx.b.const_float(*vv as f64)),
                t if t.is_int() && t.bits() != 64 => {
                    // const_int は常に i64 で生成されるため、式の型幅へ合わせる
                    let iv = ctx.b.const_int(*vv);
                    self.cast_value_simple(ctx, IrType::I64, iv, t.clone())
                }
                _ => Ok(ctx.b.const_int(*vv)),
            },
            TExprKind::Float(f) => Ok(ctx.b.const_float(*f)),
            TExprKind::Bool(b) => Ok(ctx.b.const_bool(*b)),
            TExprKind::Null => Ok(ctx.b.const_null()),
            TExprKind::Str(s) => Ok(self.str_cell(ctx, s)),
            TExprKind::Local(name) => {
                let slot = ctx.locals.get(name).cloned().ok_or_else(|| format!("unbound local `{name}`"))?;
                self.read_cell(ctx, slot.cell, &slot.ty)
            }
            TExprKind::Unary(op, inner) => {
                let ity = self.conv(&inner.ty)?;
                match op {
                    UnOp::AddrOf => self.lvalue_addr(ctx, inner),
                    UnOp::Deref => {
                        require_unsafe_semantics_ok(inner);
                        let pv = self.expr(ctx, inner)?;
                        if is_val_ptr(&ity) {
                            Ok(pv) // ポインタがそのまま実体
                        } else {
                            Ok(ctx.b.load(pv, ity))
                        }
                    }
                    UnOp::Neg => {
                        let iv = self.expr(ctx, inner)?;
                        let z = const_zero(&mut ctx.b, &ity);
                        if ity.is_float() || !e.checked {
                            Ok(ctx.b.binop(IrBin::Sub, ity, z, iv))
                        } else {
                            let (val, ovf) = ctx.b.checked_bin(IrBin::Sub, ity.clone(), z, iv);
                            self.overflow_branch(ctx, ovf)?;
                            Ok(val)
                        }
                    }
                    UnOp::Not => {
                        let iv = self.expr(ctx, inner)?;
                        let f = ctx.b.const_bool(false);
                        Ok(ctx.b.cmp(IrPred::Eq, IrType::Bool, iv, f))
                    }
                }
            }
            TExprKind::Binary(op, l, r) => self.binary_expr(ctx, *op, l, r, e.checked),
            TExprKind::Cast(inner) => {
                let iv = self.expr(ctx, inner)?;
                self.cast_value(ctx, inner, iv, ty.clone())
            }
            TExprKind::Call(callee, args) => self.call_expr(ctx, callee, args, ty, &e.ty),
            TExprKind::Index { base, index } => {
                let bty = self.conv(&base.ty)?;
                let bv = self.expr(ctx, base)?;
                let iv = self.expr(ctx, index)?;
                let elem = pointee(&bty)
                    .or(match &bty {
                        IrType::Array(t, _) => Some((**t).clone()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("cannot index `{}`", base.ty.display()))?;
                self.bounds_check(ctx, &bty, &base.ty, bv, iv)?;
                let addr = ctx.b.elem_addr(bv, iv, elem.clone());
                if is_val_ptr(&elem) {
                    // 要素セルからポインタをロード
                    Ok(ctx.b.load(addr, cell_ty(&elem)))
                } else {
                    Ok(ctx.b.load(addr, elem))
                }
            }
            TExprKind::Field { base, index } => {
                let sid = self.mono_struct_of(&base.ty)?;
                let bv = self.expr(ctx, base)?;
                let fty = self.out.structs[sid].fields[*index].1.clone();
                let addr = ctx.b.field_addr(bv, sid, *index);
                if is_val_ptr(&fty) {
                    Ok(ctx.b.load(addr, cell_ty(&fty)))
                } else {
                    Ok(ctx.b.load(addr, fty))
                }
            }
            TExprKind::Deref(inner) => {
                let ity = self.conv(&inner.ty)?;
                let pt = pointee(&ity).ok_or("deref of non-pointer")?;
                let pv = self.expr(ctx, inner)?;
                if is_val_ptr(&pt) {
                    Ok(pv)
                } else {
                    Ok(ctx.b.load(pv, pt))
                }
            }
            TExprKind::AddrOf(inner) => self.lvalue_addr(ctx, inner),
            TExprKind::StructLit { mangled, fields } => {
                let sid = self.struct_by_mangled(mangled)?;
                let sty = IrType::Struct(sid);
                let buf = ctx.b.alloca(sty.clone());
                for (fi, fe) in fields {
                    let fv = self.bind_value(ctx, fe)?;
                    let fty = self.out.structs[sid].fields[*fi].1.clone();
                    let ct = cell_ty(&fty);
                    let addr = ctx.b.field_addr(buf, sid, *fi);
                    let rc = is_rc_ty(&fe.ty);
                    let tr = matches!(fe.kind, TExprKind::Call(..));
                    self.write_cell(ctx, addr, fv, &ct, rc, tr, true);
                }
                Ok(buf)
            }
            TExprKind::VariantCtor { mangled, variant, payloads } => {
                let eid = self.enum_by_mangled(mangled)?;
                let ety = IrType::Enum(eid);
                let buf = ctx.b.alloca(ety);
                let tag = ctx.b.const_int(*variant as u64);
                let tagp = ctx.b.addr_off(buf, 0);
                ctx.b.store(tagp, tag, IrType::Usize);
                for (i, pe) in payloads.iter().enumerate() {
                    let pv = self.bind_value(ctx, pe)?;
                    let pt = self.conv(&pe.ty)?;
                    let ct = cell_ty(&pt);
                    let pp = ctx.b.addr_off(buf, Self::payload_off(i));
                    let rc = is_rc_ty(&pe.ty);
                    let tr = matches!(pe.kind, TExprKind::Call(..));
                    self.write_cell(ctx, pp, pv, &ct, rc, tr, true);
                }
                Ok(buf)
            }
            TExprKind::ArrayLit(elems) => {
                let (elem, n) = match ty {
                    IrType::Array(t, n) => ((**t).clone(), *n),
                    _ => return Err("array literal with non-array type".into()),
                };
                if elems.len() as u64 != n {
                    return Err(format!("array literal has {} elements but type needs {n}", elems.len()));
                }
                let aty = IrType::Array(Rc::new(elem.clone()), n);
                let buf = ctx.b.alloca(aty);
                for (i, el) in elems.iter().enumerate() {
                    let ev = self.bind_value(ctx, el)?;
                    let ct = cell_ty(&elem);
                    let idx = ctx.b.const_int(i as u64);
                    let addr = ctx.b.elem_addr(buf, idx, elem.clone());
                    let rc = is_rc_ty(&el.ty);
                    let tr = matches!(el.kind, TExprKind::Call(..));
                    self.write_cell(ctx, addr, ev, &ct, rc, tr, true);
                }
                Ok(buf)
            }
            TExprKind::If { cond, then_body, else_body } => self.if_expr(ctx, cond, then_body, else_body, ty),
            TExprKind::Block(blk) => {
                let v = self.run_block(ctx, blk)?;
                Ok(v.unwrap_or(NO_V))
            }
            TExprKind::Match { scrutinee, arms } => self.match_expr(ctx, scrutinee, arms, ty),
            TExprKind::Try(inner) => self.try_expr(ctx, inner, ty),
            TExprKind::Props(attrs) => {
                // JSX props オブジェクト（Rc）を生成し属性をボックス化して格納
                let pty = IrType::Ptr(Rc::new(IrType::U8));
                let props =
                    ctx.b.call("__ngs_props_new", vec![], pty.clone()).ok_or("props_new failed")?;
                for (name, val) in attrs {
                    let vv = self.expr(ctx, val)?;
                    let vt = self.conv(&val.ty)?;
                    let boxed = ctx.b.box_val(vv, vt);
                    let np = ctx.b.str_lit(name);
                    let nl = ctx.b.const_int(name.len() as u64);
                    ctx.b.call("__ngs_props_set", vec![props, np, nl, boxed], IrType::Void);
                }
                Ok(props)
            }
            TExprKind::UninitPlaceholder => Ok(const_zero(&mut ctx.b, ty)),
        }
    }

    fn str_cell(&mut self, ctx: &mut FnCtx, s: &str) -> V {
        // Const::Str(id): codegen はリテラルidに対応する
        // {data:*, len:usize} 定数セルのアドレスを生成する規約
        ctx.b.str_lit(s)
    }

    fn binary_expr(
        &mut self,
        ctx: &mut FnCtx,
        op: BinOp,
        l: &TExpr,
        r: &TExpr,
        checked: bool,
    ) -> Result<V, String> {
        use BinOp::*;
        // 論理和・論理積は短絡評価（phi がないため結果セル経由）
        if op == And || op == Or {
            let cell = ctx.b.alloca(IrType::Bool);
            let lv = self.expr(ctx, l)?;
            let short_val = ctx.b.const_bool(op == Or);
            ctx.b.store(cell, short_val, IrType::Bool);
            let (rb, rl) = ctx.b.new_block("sc.rhs");
            let (eb, el) = ctx.b.new_block("sc.end");
            if op == And {
                ctx.b.cond_br(lv, &rl, &el);
            } else {
                ctx.b.cond_br(lv, &el, &rl);
            }
            ctx.b.position(rb);
            let rv = self.expr(ctx, r)?;
            ctx.b.store(cell, rv, IrType::Bool);
            ctx.b.br(&el);
            ctx.b.position(eb);
            return Ok(ctx.b.load(cell, IrType::Bool));
        }
        let lt = self.conv(&l.ty)?;
        let lv = self.expr(ctx, l)?;
        let rv = self.expr(ctx, r)?;
        match op {
            Add | Sub | Mul | Div => self.arith_checked(ctx, to_irbin(op), lt, lv, rv, checked),
            Mod => {
                if lt.is_float() {
                    return Ok(ctx.b.call("__ngs_fmod", vec![lv, rv], IrType::F64)
                        .ok_or("fmod failed")?);
                }
                self.arith_checked(ctx, IrBin::Mod, lt, lv, rv, false)
            }
            Eq | Neq => {
                if matches!(lt, IrType::Str) {
                    let eq = ctx
                        .b
                        .call("__ngs_str_eq", vec![lv, rv], IrType::Bool)
                        .ok_or("str_eq failed")?;
                    if op == Neq {
                        let f = ctx.b.const_bool(false);
                        return Ok(ctx.b.cmp(IrPred::Eq, IrType::Bool, eq, f));
                    }
                    return Ok(eq);
                }
                let pred = if op == Eq { IrPred::Eq } else { IrPred::Ne };
                Ok(ctx.b.cmp(pred, lt, lv, rv))
            }
            Lt => Ok(ctx.b.cmp(IrPred::Lt, lt, lv, rv)),
            Le => Ok(ctx.b.cmp(IrPred::Le, lt, lv, rv)),
            Gt => Ok(ctx.b.cmp(IrPred::Gt, lt, lv, rv)),
            Ge => Ok(ctx.b.cmp(IrPred::Ge, lt, lv, rv)),
            And | Or => unreachable!(),
        }
    }

    fn arith_checked(
        &mut self,
        ctx: &mut FnCtx,
        op: IrBin,
        ty: IrType,
        l: V,
        r: V,
        checked: bool,
    ) -> Result<V, String> {
        if !ty.is_float() && checked {
            let (val, ovf) = ctx.b.checked_bin(op, ty.clone(), l, r);
            self.overflow_branch(ctx, ovf)?;
            Ok(val)
        } else {
            Ok(ctx.b.binop(op, ty, l, r))
        }
    }

    /// オーバーフロー時 "integer overflow" で panic する分岐を挟む
    fn overflow_branch(&mut self, ctx: &mut FnCtx, ovf: V) -> Result<(), String> {
        let (fb, fl) = ctx.b.new_block("of.fail");
        let (ob, ol) = ctx.b.new_block("of.ok");
        ctx.b.cond_br(ovf, &fl, &ol);
        ctx.b.position(fb);
        self.emit_panic(ctx, "integer overflow")?;
        ctx.b.position(ob);
        Ok(())
    }

    fn emit_panic(&mut self, ctx: &mut FnCtx, msg: &str) -> Result<(), String> {
        // Str 値は {data,len} セルへのポインタのため、実体と長さをロードして渡す
        let p = ctx.b.str_lit(msg);
        let dp = ctx.b.addr_off(p, 0);
        let lp = ctx.b.addr_off(p, 8);
        let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
        let l = ctx.b.load(lp, IrType::Usize);
        ctx.b.call("__ngs_panic", vec![d, l], IrType::Void);
        ctx.b.unreachable();
        Ok(())
    }

    fn arith_binop(
        &mut self,
        ctx: &mut FnCtx,
        op: BinOp,
        ty: IrType,
        l: V,
        r: V,
        checked: bool,
    ) -> Result<V, String> {
        use BinOp::*;
        match op {
            Add | Sub | Mul | Div => self.arith_checked(ctx, to_irbin(op), ty, l, r, checked),
            Mod => {
                if ty.is_float() {
                    return Ok(ctx.b.call("__ngs_fmod", vec![l, r], IrType::F64).ok_or("fmod failed")?);
                }
                self.arith_checked(ctx, IrBin::Mod, ty, l, r, false)
            }
            _ => Err("invalid compound assignment operator".into()),
        }
    }

    fn cast_value(&mut self, ctx: &mut FnCtx, inner: &TExpr, v: V, to: IrType) -> Result<V, String> {
        let from = self.conv(&inner.ty)?;
        use CastKind::*;
        let r = match (&from, &to) {
            (IrType::Str, IrType::Bool) => {
                let lenp = ctx.b.addr_off(v, 8);
                let len = ctx.b.load(lenp, IrType::Usize);
                let z = ctx.b.const_int(0);
                ctx.b.cmp(IrPred::Ne, IrType::Usize, len, z)
            }
            (IrType::Str, IrType::I64) => ctx
                .b
                .call("__ngs_str_to_i64", vec![v], IrType::I64)
                .ok_or("__ngs_str_to_i64 failed")?,
            (IrType::Str, IrType::F64) => ctx
                .b
                .call("__ngs_str_to_f64", vec![v], IrType::F64)
                .ok_or("__ngs_str_to_f64 failed")?,
            (IrType::Enum(_), IrType::Usize) => {
                let tagp = ctx.b.addr_off(v, 0);
                ctx.b.load(tagp, IrType::Usize)
            }
            (a, b) if a == b => v,
            (_, t) if t.is_float() && from.is_int() => {
                ctx.b.cast(if is_signed_int(&from) { Sitofp } else { Fptoui }, v, to.clone())
            }
            (_, t) if t.is_int() && from.is_float() => ctx.b.cast(Fptosi, v, to.clone()),
            (_, t) if t.is_float() && from.is_float() => {
                ctx.b.cast(if t.bits() > from.bits() { Fpext } else { Fptrunc }, v, to.clone())
            }
            (_, t) if t.is_int() && from.is_int() => {
                let (fb, tb) = (from.bits(), t.bits());
                let kind = match fb.cmp(&tb) {
                    std::cmp::Ordering::Greater => Trunc,
                    std::cmp::Ordering::Equal => return Ok(v),
                    std::cmp::Ordering::Less => {
                        if is_signed_int(&from) {
                            Sext
                        } else {
                            Zext
                        }
                    }
                };
                ctx.b.cast(kind, v, to.clone())
            }
            (IrType::Bool, t) if t.is_int() => ctx.b.cast(BoolToInt, v, to.clone()),
            (a, IrType::Bool) if a.is_int() => ctx.b.cast(IntToBool, v, to.clone()),
            (IrType::Ptr(_), IrType::Ptr(_)) => ctx.b.bitcast(v, to.clone()),
            (IrType::Ptr(_), _) if to.is_int() => ctx.b.cast(Ptrtoint, v, to.clone()),
            (_, IrType::Ptr(_)) if from.is_int() => ctx.b.cast(Inttoptr, v, to.clone()),
            _ => return Err(format!("unsupported cast `{}` -> {:?}", inner.ty.display(), to)),
        };
        Ok(r)
    }

    fn bounds_check(&mut self, ctx: &mut FnCtx, ir_bty: &IrType, sema_bty: &Ty, base: V, idx: V) -> Result<(), String> {
        let checked = sema_checked_index(sema_bty);
        if !checked {
            return Ok(()); // 生ポインタ等（unsafe 文脈）
        }
        let len = match ir_bty {
            IrType::Array(_, n) => ctx.b.const_int(*n),
            IrType::Struct(si) if self.out.structs[*si].is_list => ctx
                .b
                .call("__ngs_list_len", vec![base], IrType::Usize)
                .ok_or("__ngs_list_len failed")?,
            _ => return Ok(()),
        };
        let bad = ctx.b.cmp(IrPred::Ge, IrType::Usize, idx, len);
        let (fb, fl) = ctx.b.new_block("oob.fail");
        let (ob, ol) = ctx.b.new_block("oob.ok");
        ctx.b.cond_br(bad, &fl, &ol);
        ctx.b.position(fb);
        self.emit_panic(ctx, "array index out of bounds")?;
        ctx.b.position(ob);
        Ok(())
    }

    // ------------------------------------------------------------------
    // if / match / try
    // ------------------------------------------------------------------

    fn if_expr(
        &mut self,
        ctx: &mut FnCtx,
        cond: &TExpr,
        then_body: &TBlock,
        else_body: &Option<Box<TExpr>>,
        ty: &IrType,
    ) -> Result<V, String> {
        let cv = self.expr(ctx, cond)?;
        let (tb, tl) = ctx.b.new_block("if.then");
        let (fb, fl) = ctx.b.new_block("if.else");
        let (eb, el) = ctx.b.new_block("if.end");
        ctx.b.cond_br(cv, &tl, &fl);

        let void = matches!(ty, IrType::Void);
        let ct = cell_ty(ty);
        let cell = if void { NO_V } else { ctx.b.alloca(ct.clone()) };

        ctx.b.position(tb);
        let tv = self.run_block(ctx, then_body)?;
        if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
            if !void {
                if let Some(v) = tv {
                    ctx.b.store(cell, v, ct.clone());
                }
            }
            ctx.b.br(&el);
        }

        ctx.b.position(fb);
        match else_body {
            Some(elsexpr) => {
                let ev = self.expr(ctx, elsexpr)?;
                if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
                    if !void && !matches!(ev, NO_V) {
                        ctx.b.store(cell, ev, ct.clone());
                    }
                    ctx.b.br(&el);
                }
            }
            None => {
                // else 無し: 条件不成立でも merge へ進む
                ctx.b.br(&el);
            }
        }

        ctx.b.position(eb);
        if void || matches!(ty, IrType::Void) {
            Ok(NO_V)
        } else {
            Ok(ctx.b.load(cell, ct))
        }
    }

    fn match_expr(
        &mut self,
        ctx: &mut FnCtx,
        scrutinee: &TExpr,
        arms: &[TArm],
        ty: &IrType,
    ) -> Result<V, String> {
        let sty = self.conv(&scrutinee.ty)?;
        let sv = self.expr(ctx, scrutinee)?;

        let void = matches!(ty, IrType::Void);
        let ct = cell_ty(ty);
        let cell = if void { NO_V } else { ctx.b.alloca(ct.clone()) };
        let (_, end_label) = ctx.b.new_block("match.end");

        // スクルティニーをメモリスロットへ正規化（各パターンはスロットから読む）
        let sl_ty = cell_ty(&sty);
        let slot = ctx.b.alloca(sl_ty.clone());
        ctx.b.store(slot, sv, sl_ty);

        // パターン束縛は借り参照（rc=false）なので、退出時はブックキープのみでよい。
        // 各アームの開始時と match 終了時にオーバーフローの束縛を切り落とす。
        let mbase = ctx.order.len();
        ctx.truncate(mbase);

        // 先頭アームのテストブロックへ遷移
        let (first_idx, first_label) = ctx.b.new_block("match.arm0");
        ctx.b.br(&first_label);
        let mut next = {
            ctx.b.position(first_idx);
            first_idx
        };

        for (i, arm) in arms.iter().enumerate() {
            ctx.b.position(next);
            ctx.truncate(mbase);
            if matches!(arm.pattern, TPattern::Wildcard) {
                // ワイルドカード: 常にマッチ（後続アームは到達不能）
                self.arm_body(ctx, &arm.body, cell, ct.clone(), void, &end_label)?;
                break;
            }
            // fail ブロック（次のアームへ / 最後は panic）
            let (nidx, nlabel);
            if i + 1 < arms.len() {
                let nb = ctx.b.new_block("match.next");
                nidx = nb.0;
                nlabel = nb.1;
            } else {
                let nb = ctx.b.new_block("match.none");
                nidx = nb.0;
                nlabel = nb.1;
            }

            // このアームが束縛する変数セルを確保
            let mut blist: Vec<(String, Ty)> = Vec::new();
            collect_pattern_bindings(&arm.pattern, &mut blist);
            let mut bcells: HashMap<String, V> = HashMap::new();
            for (name, bty) in &blist {
                let birt = self.conv(bty)?;
                let bct = cell_ty(&birt);
                let bc = ctx.b.alloca(bct.clone());
                bcells.insert(name.clone(), bc);
            }

            // パターンをコンパイル。マッチした続きのブロックを返す
            let matched = self.compile_pattern(ctx, slot, &sty, &arm.pattern, &bcells, &nlabel)?;
            ctx.b.position(matched);

            // ガード
            if let Some(guard) = &arm.guard {
                let gv = self.expr(ctx, guard)?;
                let (bidx, blabel) = ctx.b.new_block("match.body");
                ctx.b.cond_br(gv, &blabel, &nlabel);
                ctx.b.position(bidx);
            }
            self.arm_body(ctx, &arm.body, cell, ct.clone(), void, &end_label)?;

            let idx = ctx
                .b
                .func
                .find_block(&nlabel)
                .ok_or_else(|| "internal error: missing match.next block".to_string())?;
            ctx.b.position(idx);
            let _ = nidx;
            next = idx;
        }

        // どの腕も通らなかった場合（sema で網羅性は保証済みだが安全のため）
        ctx.truncate(mbase);
        if !matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
            // 最後のアームの fail ブロック（残っていれば）で panic
            let cur = ctx.b.cur;
            let is_fresh = {
                let blk = &ctx.b.func.blocks[cur];
                blk.insts.is_empty() && matches!(blk.term, Term::Unreachable)
            };
            if is_fresh {
                self.emit_panic(ctx, "non-exhaustive match")?;
            }
        }

        let eidx = ctx
            .b
            .func
            .find_block(&end_label)
            .ok_or_else(|| "internal error: missing match.end block".to_string())?;
        ctx.b.position(eidx);

        if void {
            Ok(NO_V)
        } else {
            if matches!(ctx.b.func.blocks[eidx].term, Term::Unreachable) {}
            Ok(ctx.b.load(cell, ct))
        }
    }

    /// パターンを IR へコンパイルする。
    ///
    /// - `slot` は調査対象の値が入るメモリスロット（セル型 = cell_ty(vt)）。
    /// - `vt` はスロットに入る値の IR 型（aggregate は構造体型）。
    /// - マッチしなければ `fail` ラベルへ分岐する。
    /// - マッチしたら（束縛も登録済みで）継続ブロックのインデックスを返す。
    fn compile_pattern(
        &mut self,
        ctx: &mut FnCtx,
        slot: V,
        vt: &IrType,
        pat: &TPattern,
        bcells: &HashMap<String, V>,
        fail: &str,
    ) -> Result<usize, String> {
        match pat {
            TPattern::Wildcard => Ok(ctx.b.cur),
            TPattern::Int(vv) => {
                let pv = const_int_for(&mut ctx.b, vt, *vv as u64);
                let val = ctx.b.load(slot, vt.clone());
                let eq = ctx.b.cmp(IrPred::Eq, vt.clone(), val, pv);
                let (t_idx, t_label) = ctx.b.new_block("m.i");
                ctx.b.cond_br(eq, &t_label, fail);
                ctx.b.position(t_idx);
                Ok(t_idx)
            }
            TPattern::Bool(vv) => {
                let val = ctx.b.load(slot, IrType::Bool);
                let pv = ctx.b.const_bool(*vv);
                let eq = ctx.b.cmp(IrPred::Eq, IrType::Bool, val, pv);
                let (t_idx, t_label) = ctx.b.new_block("m.b");
                ctx.b.cond_br(eq, &t_label, fail);
                ctx.b.position(t_idx);
                Ok(t_idx)
            }
            TPattern::Str(ss) => {
                let val = ctx.b.load(slot, IrType::Ptr(Rc::new(vt.clone())));
                let lit = ctx.b.str_lit(ss);
                let eq = ctx
                    .b
                    .call("__ngs_str_eq", vec![val, lit], IrType::Bool)
                    .ok_or("__ngs_str_eq failed")?;
                let (t_idx, t_label) = ctx.b.new_block("m.s");
                ctx.b.cond_br(eq, &t_label, fail);
                ctx.b.position(t_idx);
                Ok(t_idx)
            }
            TPattern::Binding { name, ty } => {
                let cell = bcells
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("internal: missing binding cell for `{name}`"))?;
                let birt = self.conv(ty)?;
                let bct = cell_ty(&birt);
                let bound = if is_val_ptr(vt) {
                    ctx.b.load(slot, IrType::Ptr(Rc::new(vt.clone())))
                } else {
                    ctx.b.load(slot, vt.clone())
                };
                ctx.b.store(cell, bound, bct);
                // 借り参照（rc=false）。元のペイロード/スロットが所有権を保持
                ctx.declare(
                    name.clone(),
                    LocalSlot { cell, ty: birt, rc: false, list_elem_size: None },
                );
                Ok(ctx.b.cur)
            }
            TPattern::Range { lo, hi, inclusive } => {
                let val = ctx.b.load(slot, vt.clone());
                let lo_c = const_int_for(&mut ctx.b, vt, *lo as u64);
                let hi_c = const_int_for(&mut ctx.b, vt, *hi as u64);
                let ge = ctx.b.cmp(IrPred::Ge, vt.clone(), val, lo_c);
                let (r1_idx, r1_label) = ctx.b.new_block("m.r1");
                let (f_idx, f_label) = ctx.b.new_block("m.rx");
                ctx.b.cond_br(ge, &r1_label, &f_label);
                ctx.b.position(f_idx);
                ctx.b.br(fail);
                ctx.b.position(r1_idx);
                let lt = if *inclusive {
                    ctx.b.cmp(IrPred::Le, vt.clone(), val, hi_c)
                } else {
                    ctx.b.cmp(IrPred::Lt, vt.clone(), val, hi_c)
                };
                let (t_idx, t_label) = ctx.b.new_block("m.r");
                ctx.b.cond_br(lt, &t_label, fail);
                ctx.b.position(t_idx);
                Ok(t_idx)
            }
            TPattern::Variant { variant, fields, field_tys, .. } => {
                let obj = ctx.b.load(slot, IrType::Ptr(Rc::new(vt.clone())));
                let tagp = ctx.b.addr_off(obj, 0);
                let tag = ctx.b.load(tagp, IrType::Usize);
                let want = ctx.b.const_int(*variant as u64);
                let eq = ctx.b.cmp(IrPred::Eq, IrType::Usize, tag, want);
                let (t_idx, t_label) = ctx.b.new_block("m.v");
                ctx.b.cond_br(eq, &t_label, fail);
                ctx.b.position(t_idx);
                let mut cur = t_idx;
                for (i, fp) in fields.iter().enumerate() {
                    let fslot = ctx.b.addr_off(obj, Self::payload_off(i));
                    let firt = self.conv(&field_tys[i])?;
                    cur = self.compile_pattern(ctx, fslot, &firt, fp, bcells, fail)?;
                }
                Ok(cur)
            }
            TPattern::Or(alts) => {
                let (m_idx, m_label) = ctx.b.new_block("m.or");
                let mut entries: Vec<(usize, String)> = (0..alts.len())
                    .map(|_| ctx.b.new_block("m.alt"))
                    .collect();
                let mut conts = Vec::new();
                for (i, ap) in alts.iter().enumerate() {
                    let fail_lbl = if i + 1 < alts.len() {
                        entries[i + 1].1.clone()
                    } else {
                        fail.to_string()
                    };
                    if i == 0 {
                        // 先頭 alternative は現在のブロックでコンパイル
                        let c = self.compile_pattern(ctx, slot, vt, ap, bcells, &fail_lbl)?;
                        conts.push(c);
                    } else {
                        ctx.b.position(entries[i].0);
                        let c = self.compile_pattern(ctx, slot, vt, ap, bcells, &fail_lbl)?;
                        conts.push(c);
                    }
                }
                for c in conts {
                    ctx.b.position(c);
                    ctx.b.br(&m_label);
                }
                ctx.b.position(m_idx);
                Ok(m_idx)
            }
        }
    }

    fn arm_body(
        &mut self,
        ctx: &mut FnCtx,
        body: &TExpr,
        cell: V,
        ct: IrType,
        void: bool,
        end: &str,
    ) -> Result<(), String> {
        let base = ctx.order.len();
        self.arm_body_scoped(ctx, body, cell, ct, void, end, base)
    }

    fn arm_body_scoped(
        &mut self,
        ctx: &mut FnCtx,
        body: &TExpr,
        cell: V,
        ct: IrType,
        void: bool,
        end: &str,
        base: usize,
    ) -> Result<(), String> {
        let v = self.expr(ctx, body)?;
        if !void && !matches!(v, NO_V) {
            ctx.b.store(cell, v, ct);
        }
        self.cleanup_locals(ctx, base);
        if matches!(ctx.b.func.blocks[ctx.b.cur].term, Term::Unreachable) {
            ctx.b.br(end);
        }
        Ok(())
    }

    fn try_expr(&mut self, ctx: &mut FnCtx, inner: &TExpr, ty: &IrType) -> Result<V, String> {
        // `try expr` : expr は Result<T,E>。Err なら関数から Err をそのまま返す。
        let rv = self.expr(ctx, inner)?;
        let tagp = ctx.b.addr_off(rv, 0);
        let tag = ctx.b.load(tagp, IrType::Usize);
        let zero = ctx.b.const_int(0); // Ok
        let ok = ctx.b.cmp(IrPred::Eq, IrType::Usize, tag, zero);
        let (_errb, el) = ctx.b.new_block("try.err");
        let (okb, ol) = ctx.b.new_block("try.ok");
        ctx.b.cond_br(ok, &ol, &el);
        let _ = ol;
        ctx.b.position(_errb);
        // Err 返却: 戻り値型が同一 Result である前提でタグを Err に付け替えて返す
        let retcell = ctx.b.alloca(ty.clone());
        ctx.b.copy_agg(retcell, rv, ty.clone());
        let rtagp = ctx.b.addr_off(retcell, 0);
        let one = ctx.b.const_int(1); // Err
        ctx.b.store(rtagp, one, IrType::Usize);
        ctx.b.ret(Some(retcell));
        ctx.b.position(okb);
        // Ok ペイロードの取り出し
        let payload_ty = self.result_ok_payload(ty)?;
        let pp = ctx.b.addr_off(rv, Self::payload_off(0));
        if is_val_ptr(&payload_ty) {
            Ok(ctx.b.load(pp, cell_ty(&payload_ty)))
        } else {
            Ok(ctx.b.load(pp, payload_ty))
        }
    }

    fn result_ok_payload(&self, ty: &IrType) -> Result<IrType, String> {
        if let IrType::Enum(eid) = ty {
            let e = &self.out.enums[*eid];
            Ok(e.variants.first().and_then(|v| v.first()).cloned().unwrap_or(IrType::Void))
        } else {
            Err("try requires a Result value".into())
        }
    }

    // ------------------------------------------------------------------
    // 呼び出し / 組込み
    // ------------------------------------------------------------------

    fn call_expr(
        &mut self,
        ctx: &mut FnCtx,
        callee: &Callee,
        args: &[TExpr],
        ty: &IrType,
        sema_ty: &Ty,
    ) -> Result<V, String> {
        match callee {
            Callee::Direct(mangled) => {
                let argv = self.eval_args(ctx, args)?;
                Ok(ctx.b.call(mangled, argv, ty.clone()).unwrap_or(NO_V))
            }
            Callee::Extern(name) => {
                let argv = self.eval_args(ctx, args)?;
                Ok(ctx.b.call(name, argv, ty.clone()).unwrap_or(NO_V))
            }
            Callee::Intrinsic(intr) => self.intrinsic(ctx, intr.clone(), args, ty, sema_ty),
        }
    }

    fn eval_args(&mut self, ctx: &mut FnCtx, args: &[TExpr]) -> Result<Vec<V>, String> {
        args.iter().map(|a| self.expr(ctx, a)).collect()
    }

    fn intrinsic(
        &mut self,
        ctx: &mut FnCtx,
        intr: Intrinsic,
        args: &[TExpr],
        _ret: &IrType,
        sema_ret: &Ty,
    ) -> Result<V, String> {
        match intr {
            Intrinsic::Print { newline, value_ty } => {
                let v = self.expr(ctx, &args[0])?;
                let vt = self.conv(&value_ty)?;
                let name = |base: &str| if newline { format!("__ngs_println_{base}") } else { format!("__ngs_print_{base}") };
                if matches!(vt, IrType::Str) {
                    let dp = ctx.b.addr_off(v, 0);
                    let lp = ctx.b.addr_off(v, 8);
                    let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                    let l = ctx.b.load(lp, IrType::Usize);
                    ctx.b.call(&name("str"), vec![d, l], IrType::Void);
                } else if vt.is_float() {
                    let f = self.cast_value_simple(ctx, vt, v, IrType::F64)?;
                    ctx.b.call(&name("f64"), vec![f], IrType::Void);
                } else if value_ty == Ty::Bool {
                    ctx.b.call(&name("bool"), vec![v], IrType::Void);
                } else if vt.is_int() {
                    let i = self.cast_value_simple(ctx, vt, v, IrType::I64)?;
                    ctx.b.call(&name("i64"), vec![i], IrType::Void);
                } else {
                    return Err(format!("print cannot display `{}`", value_ty.display()));
                }
                Ok(NO_V)
            }
            Intrinsic::PrintFStr { newline, parts } => {
                for part in &parts {
                    match part {
                        TFStringPart::Text(s) => {
                            let v = ctx.b.str_lit(s);
                            let dp = ctx.b.addr_off(v, 0);
                            let lp = ctx.b.addr_off(v, 8);
                            let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                            let l = ctx.b.load(lp, IrType::Usize);
                            ctx.b.call("__ngs_print_str", vec![d, l], IrType::Void);
                        }
                        TFStringPart::Expr(te) => {
                            let v = self.expr(ctx, te)?;
                            let vt = self.conv(&te.ty)?;
                            if matches!(vt, IrType::Str) {
                                let dp = ctx.b.addr_off(v, 0);
                                let lp = ctx.b.addr_off(v, 8);
                                let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                                let l = ctx.b.load(lp, IrType::Usize);
                                ctx.b.call("__ngs_print_str", vec![d, l], IrType::Void);
                            } else if te.ty == Ty::Bool {
                                ctx.b.call("__ngs_print_bool", vec![v], IrType::Void);
                            } else if vt.is_float() {
                                let f = self.cast_value_simple(ctx, vt, v, IrType::F64)?;
                                ctx.b.call("__ngs_print_f64", vec![f], IrType::Void);
                            } else if vt.is_int() {
                                let i = self.cast_value_simple(ctx, vt, v, IrType::I64)?;
                                ctx.b.call("__ngs_print_i64", vec![i], IrType::Void);
                            } else {
                                return Err(format!("cannot interpolate `{}`", te.ty.display()));
                            }
                        }
                    }
                }
                if newline {
                    let v = ctx.b.str_lit("\n");
                    let dp = ctx.b.addr_off(v, 0);
                    let lp = ctx.b.addr_off(v, 8);
                    let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                    let l = ctx.b.load(lp, IrType::Usize);
                    ctx.b.call("__ngs_print_str", vec![d, l], IrType::Void);
                }
                Ok(NO_V)
            }
            Intrinsic::Panic => {
                let v = self.expr(ctx, &args[0])?;
                let dp = ctx.b.addr_off(v, 0);
                let lp = ctx.b.addr_off(v, 8);
                let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                let l = ctx.b.load(lp, IrType::Usize);
                ctx.b.call("__ngs_panic", vec![d, l], IrType::Void);
                ctx.b.unreachable();
                Ok(NO_V)
            }
            Intrinsic::Abort => {
                ctx.b.call("__ngs_abort", vec![], IrType::Void);
                ctx.b.unreachable();
                Ok(NO_V)
            }
            Intrinsic::SizeOfStr => Ok(ctx.b.const_int(16)),
            Intrinsic::Len => {
                let v = self.expr(ctx, &args[0])?;
                match &args[0].ty {
                    Ty::Array(_, n) => Ok(ctx.b.const_int(*n)),
                    Ty::Str => {
                        let lp = ctx.b.addr_off(v, 8);
                        Ok(ctx.b.load(lp, IrType::Usize))
                    }
                    other => Err(format!("len requires array or string, got `{}`", other.display())),
                }
            }
            Intrinsic::ListNew => {
                let esz = self.list_elem_size(args, sema_ret)?;
                let s = ctx.b.const_int(esz.max(1));
                Ok(ctx
                    .b
                    .call("__ngs_list_new", vec![s], IrType::Ptr(Rc::new(IrType::U8)))
                    .ok_or("__ngs_list_new failed")?)
            }
            Intrinsic::ListPush => {
                let list = self.expr(ctx, &args[0])?;
                let esz = self.list_elem_size(args, sema_ret)?;
                let s = ctx.b.const_int(esz.max(1));
                let slot = ctx
                    .b
                    .call("__ngs_list_push", vec![list, s], IrType::Ptr(Rc::new(IrType::U8)))
                    .ok_or("__ngs_list_push failed")?;
                if args.len() > 1 && !matches!(args[1].kind, TExprKind::UninitPlaceholder) {
                    let v = self.bind_value(ctx, &args[1])?;
                    let et = self.conv(&args[1].ty)?;
                    let rc = is_rc_ty(&args[1].ty);
                    let tr = matches!(args[1].kind, TExprKind::Call(..));
                    // push スロットは新規領域
                    self.write_cell(ctx, slot, v, &cell_ty(&et), rc, tr, true);
                }
                Ok(NO_V)
            }
            Intrinsic::ListGet => {
                let list = self.expr(ctx, &args[0])?;
                let idx = self.expr(ctx, &args[1])?;
                self.check_list_bounds(ctx, list, idx)?;
                let at = ctx
                    .b
                    .call("__ngs_list_at", vec![list, idx], IrType::Ptr(Rc::new(IrType::U8)))
                    .ok_or("__ngs_list_at failed")?;
                let et = self.elem_ty_of(&args[0].ty)?;
                if is_val_ptr(&et) {
                    Ok(ctx.b.load(at, cell_ty(&et)))
                } else {
                    Ok(ctx.b.load(at, et))
                }
            }
            Intrinsic::ListSet => {
                let list = self.expr(ctx, &args[0])?;
                let idx = self.expr(ctx, &args[1])?;
                self.check_list_bounds(ctx, list, idx)?;
                let at = ctx
                    .b
                    .call("__ngs_list_at", vec![list, idx], IrType::Ptr(Rc::new(IrType::U8)))
                    .ok_or("__ngs_list_at failed")?;
                let val = self.bind_value(ctx, &args[2])?;
                let et = self.elem_ty_of(&args[0].ty)?;
                let rc = is_rc_ty(&args[2].ty);
                let tr = matches!(args[2].kind, TExprKind::Call(..));
                self.write_cell(ctx, at, val, &cell_ty(&et), rc, tr, false);
                Ok(NO_V)
            }
            Intrinsic::ListLen => {
                let list = self.expr(ctx, &args[0])?;
                Ok(ctx
                    .b
                    .call("__ngs_list_len", vec![list], IrType::Usize)
                    .ok_or("__ngs_list_len failed")?)
            }
            Intrinsic::RcNew => {
                let pt = self.conv(&args[0].ty)?;
                let dsz = self.out.size_of(&pt).max(1);
                let szv = ctx.b.const_int(dsz);
                let obj = ctx
                    .b
                    .call("__ngs_rc_new", vec![szv], IrType::Ptr(Rc::new(IrType::U8)))
                    .ok_or("__ngs_rc_new failed")?;
                // Rc オブジェクトレイアウト: {count@0, size@8, data@16..}
                let data = ctx.b.addr_off(obj, 16);
                let v = self.expr(ctx, &args[0])?;
                ctx.b.store(data, v, pt);
                Ok(obj)
            }
            Intrinsic::RcGet => {
                let obj = self.expr(ctx, &args[0])?;
                let inner = match &args[0].ty {
                    Ty::RcT(t) => (**t).clone(),
                    other => return Err(format!("RcGet on non-Rc `{}`", other.display())),
                };
                let it = self.conv(&inner)?;
                let data = ctx.b.addr_off(obj, 16);
                if is_val_ptr(&it) {
                    Ok(ctx.b.load(data, cell_ty(&it)))
                } else {
                    Ok(ctx.b.load(data, it))
                }
            }
            Intrinsic::StrEq => {
                let a = self.expr(ctx, &args[0])?;
                let b = self.expr(ctx, &args[1])?;
                Ok(ctx.b.call("__ngs_str_eq", vec![a, b], IrType::Bool).ok_or("__ngs_str_eq failed")?)
            }
            Intrinsic::PropsNew => {
                // args: [タグ文字列, Props, 子供...]
                let props = self.expr(ctx, &args[1])?;
                let tag = self.expr(ctx, &args[0])?;
                let dp = ctx.b.addr_off(tag, 0);
                let lp = ctx.b.addr_off(tag, 8);
                let d = ctx.b.load(dp, IrType::Ptr(Rc::new(IrType::U8)));
                let l = ctx.b.load(lp, IrType::Usize);
                ctx.b.call("__ngs_props_tag", vec![props, d, l], IrType::Void);
                for child in &args[2..] {
                    let cv = self.expr(ctx, child)?;
                    let ct = self.conv(&child.ty)?;
                    let boxed = ctx.b.box_val(cv, ct);
                    ctx.b.call("__ngs_props_add_child", vec![props, boxed], IrType::Void);
                }
                Ok(NO_V)
            }
            Intrinsic::PropsSet { .. } => {
                let props = self.expr(ctx, &args[0])?;
                let name = self.expr(ctx, &args[1])?;
                let val = self.expr(ctx, &args[2])?;
                let vt = self.conv(&args[2].ty)?;
                let boxed = ctx.b.box_val(val, vt);
                ctx.b.call("__ngs_props_set", vec![props, name, boxed], IrType::Void);
                Ok(NO_V)
            }
            Intrinsic::BoxAny { value_ty } => {
                let v = self.expr(ctx, &args[0])?;
                let vt = self.conv(&value_ty)?;
                Ok(ctx.b.box_val(v, vt))
            }
        }
    }

    fn cast_value_simple(&self, ctx: &mut FnCtx, from: IrType, v: V, to: IrType) -> Result<V, String> {
        if from == to {
            return Ok(v);
        }
        use CastKind::*;
        let kind = if from.is_int() && to.is_int() {
            if from.bits() > to.bits() {
                Trunc
            } else if is_signed_int(&from) {
                Sext
            } else {
                Zext
            }
        } else if from.is_int() && to.is_float() {
            Sitofp
        } else if from.is_float() && to.is_float() {
            if to.bits() > from.bits() {
                Fpext
            } else {
                Fptrunc
            }
        } else {
            return Err("invalid print cast".into());
        };
        Ok(ctx.b.cast(kind, v, to))
    }

    /// List<T> の要素スロットサイズ。引数の List 型式、だめなら戻り値型から推定する。
    /// スロットは「セル」単位（集約・Str はポインタ1個分）。
    fn list_elem_size(&self, args: &[TExpr], sema_ret: &Ty) -> Result<u64, String> {
        let mut candidates: Vec<&Ty> = args.iter().map(|a| &a.ty).collect();
        candidates.push(sema_ret);
        for t in candidates {
            if let Ty::Struct(id, subs) = t {
                if *id == ngs_sema::BUILTIN_LIST {
                    let et = self.conv(subs.first().unwrap_or(&Ty::Void))?;
                    return Ok(self.out.cell_of(&et));
                }
            }
        }
        Err("cannot determine list element size".into())
    }

    fn elem_ty_of(&self, list_ty: &Ty) -> Result<IrType, String> {
        match list_ty {
            Ty::Struct(id, subs) if *id == ngs_sema::BUILTIN_LIST => {
                let et = self.conv(subs.first().unwrap_or(&Ty::Void))?;
                Ok(et)
            }
            other => Err(format!("not a list: `{}`", other.display())),
        }
    }

    fn check_list_bounds(&mut self, ctx: &mut FnCtx, list: V, idx: V) -> Result<(), String> {
        let len = ctx
            .b
            .call("__ngs_list_len", vec![list], IrType::Usize)
            .ok_or("__ngs_list_len failed")?;
        let bad = ctx.b.cmp(IrPred::Ge, IrType::Usize, idx, len);
        let (fb, fl) = ctx.b.new_block("oob.fail");
        let (ob, ol) = ctx.b.new_block("oob.ok");
        ctx.b.cond_br(bad, &fl, &ol);
        ctx.b.position(fb);
        self.emit_panic(ctx, "array index out of bounds")?;
        ctx.b.position(ob);
        Ok(())
    }

    // ------------------------------------------------------------------
    // セル入出力・束縛
    // ------------------------------------------------------------------

    fn read_cell(&self, ctx: &mut FnCtx, cell: V, logical: &IrType) -> Result<V, String> {
        let ct = cell_ty(logical);
        Ok(ctx.b.load(cell, ct))
    }

    /// セルへ書き込む。Rc なら inc/dec を調整する。
    /// fresh == true のときセルは未初期化（古い値の dec を行わない）。
    #[allow(clippy::too_many_arguments)]
    fn write_cell(
        &mut self,
        ctx: &mut FnCtx,
        addr: V,
        v: V,
        ct: &IrType,
        rc: bool,
        transferred: bool,
        fresh: bool,
    ) {
        if rc {
            let old = if fresh { None } else { Some(ctx.b.load(addr, ct.clone())) };
            if !transferred {
                ctx.b.rc_inc(v);
            }
            ctx.b.store(addr, v, ct.clone());
            if let Some(o) = old {
                ctx.b.rc_dec(o);
            }
        } else {
            ctx.b.store(addr, v, ct.clone());
        }
    }

    /// 式を評価してセル書き込み用の値にする（集約は原義コピー、Rc は移転判定込み）
    fn write_cell_expr(&mut self, ctx: &mut FnCtx, addr: V, value: &TExpr, ct: &IrType, fresh: bool) {
        let v = match self.bind_value(ctx, value) {
            Ok(v) => v,
            Err(_) => return,
        };
        let rc = is_rc_ty(&value.ty);
        let transferred = matches!(value.kind, TExprKind::Call(..));
        self.write_cell(ctx, addr, v, ct, rc, transferred, fresh);
    }

    /// 束縛用の値を用意する。集約（Str以外）で新規生成値でない場合はコピーを作る。
    fn bind_value(&mut self, ctx: &mut FnCtx, e: &TExpr) -> Result<V, String> {
        let ty = self.conv(&e.ty)?;
        let v = self.expr(ctx, e)?;
        if is_val_ptr(&ty) && !matches!(ty, IrType::Str) && !is_fresh(e) {
            let tmp = ctx.b.alloca(ty.clone());
            ctx.b.copy_agg(tmp, v, ty);
            return Ok(tmp);
        }
        Ok(v)
    }

    // ------------------------------------------------------------------
    // 左辺値
    // ------------------------------------------------------------------

    fn lvalue_addr(&mut self, ctx: &mut FnCtx, e: &TExpr) -> Result<V, String> {
        match &e.kind {
            TExprKind::Local(name) => {
                let slot = ctx.locals.get(name).cloned().ok_or_else(|| format!("unbound local `{name}`"))?;
                Ok(slot.cell)
            }
            TExprKind::Index { base, index } => {
                let bty = self.conv(&base.ty)?;
                let bv = self.expr(ctx, base)?;
                let iv = self.expr(ctx, index)?;
                let elem = pointee(&bty)
                    .or(match &bty {
                        IrType::Array(t, _) => Some((**t).clone()),
                        _ => None,
                    })
                    .ok_or_else(|| format!("cannot index `{}`", base.ty.display()))?;
                self.bounds_check(ctx, &bty, &base.ty, bv, iv)?;
                Ok(ctx.b.elem_addr(bv, iv, elem))
            }
            TExprKind::Field { base, index } => {
                let sid = self.mono_struct_of(&base.ty)?;
                let bv = self.expr(ctx, base)?;
                Ok(ctx.b.field_addr(bv, sid, *index))
            }
            TExprKind::Deref(inner) => {
                let ity = self.conv(&inner.ty)?;
                pointee(&ity).ok_or_else(|| format!("deref of non-pointer `{}`", inner.ty.display()))?;
                self.expr(ctx, inner)
            }
            _ => Err(format!("expression is not assignable ({:?})", e.kind)),
        }
    }
}

// ---------------------------------------------------------------------------
// 補助関数
// ---------------------------------------------------------------------------

fn is_signed_int(t: &IrType) -> bool {
    matches!(t, IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::Isize)
}

fn to_irbin(op: BinOp) -> IrBin {
    match op {
        BinOp::Add => IrBin::Add,
        BinOp::Sub => IrBin::Sub,
        BinOp::Mul => IrBin::Mul,
        BinOp::Div => IrBin::Div,
        BinOp::Mod => IrBin::Mod,
        _ => unreachable!(),
    }
}

fn const_zero(b: &mut FnBuilder, t: &IrType) -> V {
    if t.is_float() {
        b.const_float(0.0)
    } else if matches!(t, IrType::Bool) {
        b.const_bool(false)
    } else if let IrType::Ptr(_) = t {
        let z = b.const_int(0);
        b.bitcast(z, t.clone())
    } else {
        b.const_int(0)
    }
}

/// パターン比較用の定数（scrutinee 型に合わせる）
fn const_int_for(b: &mut FnBuilder, sty: &IrType, v: u64) -> V {
    if sty.is_float() {
        b.const_float(v as f64)
    } else {
        b.const_int(v)
    }
}

/// パターンが束縛する変数名と型を（名前で重複除去して）収集する。
fn collect_pattern_bindings(pat: &TPattern, out: &mut Vec<(String, Ty)>) {
    fn rec(pat: &TPattern, out: &mut Vec<(String, Ty)>, seen: &mut HashSet<String>) {
        match pat {
            TPattern::Binding { name, ty } => {
                if seen.insert(name.clone()) {
                    out.push((name.clone(), ty.clone()));
                }
            }
            TPattern::Variant { fields, .. } => {
                for f in fields {
                    rec(f, out, seen);
                }
            }
            TPattern::Or(alts) => {
                for a in alts {
                    rec(a, out, seen);
                }
            }
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    rec(pat, out, &mut seen);
}

/// 生ポインタへの添字は unsafe 文脈で境界検査なし。それ以外は検査あり。
fn sema_checked_index(base_sema: &Ty) -> bool {
    !matches!(base_sema, Ty::Ptr(_))
}

fn require_unsafe_semantics_ok(_e: &TExpr) {
    // unsafe 境界は sema が保証済み
}

#[cfg(test)]
mod tests {
    use super::*;
    use ngs_sema::check;

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

    fn run(src: &str) -> Result<IrProgram, String> {
        let file = ngs_parser::parse_source(src, "test.ngs").map_err(|e| format!("{e:?}"))?;
        let typed = check(&file).map_err(|es| es.iter().map(|e| e.msg.clone()).collect::<Vec<_>>().join("; "))?;
        lower(&typed)
    }

    #[test]
    fn smoke_lower_pipeline() {
        let prog = run(SRC).expect("lowering should succeed");
        let text = crate::dump::dump_program(&prog);
        assert!(prog.funcs.iter().any(|f| f.name.contains("answer")));
        // main 相当（is_user_main）が無いソースなので main_name は None でもよい
        let _ = prog.main_name;
        assert!(!prog.strings.is_empty(), "string literals should be interned");
        assert!(text.contains("fn "), "dump should contain functions");
        println!("{}", text);
    }

    #[test]
    fn reject_missing_return_path() {
        let src = r#"
fn f(x: bool) -> i32 {
    if x { return 1; }
}
"#;
        assert!(run(src).is_err());
    }

    /// 複数関数がそれぞれ文字列リテラルを持つとき、Const::Str の id は
    /// グローバルプールの正しい要素を指すこと（従来はビルダ毎に 0 起点で衝突していた）
    #[test]
    fn string_ids_are_global_across_functions() {
        let src = r#"
fn a() { print("AAA"); }
fn b() { print("BBB"); }
fn main() { a(); b(); }
"#;
        let prog = run(src).expect("lower");
        let mut found = vec![];
        for f in &prog.funcs {
            for blk in &f.blocks {
                for inst in &blk.insts {
                    if let crate::Inst::Const { val: crate::Const::Str(id), .. } = inst {
                        found.push((*id as usize, f.name.clone()));
                    }
                }
            }
        }
        // 関数 a → "AAA"、b → "BBB" が正しく対応するか
        let lookup =
            |name: &str| found.iter().find(|(_, f)| *f == name).map(|(id, _)| prog.strings[*id].content.clone());
        assert_eq!(lookup("a").as_deref(), Some("AAA"), "pool={:?}", prog.strings.iter().map(|s| &s.content).collect::<Vec<_>>());
        assert_eq!(lookup("b").as_deref(), Some("BBB"));
    }
}
