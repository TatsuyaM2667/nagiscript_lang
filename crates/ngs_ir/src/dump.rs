//! IR のテキストダンプ（デバッグ用）

use crate::*;

pub fn dump_program(p: &IrProgram) -> String {
    let mut s = String::new();
    for st in &p.structs {
        let fields: Vec<String> =
            st.fields.iter().map(|(n, t)| format!("{n}: {t:?}")).collect();
        s.push_str(&format!(
            "struct {} {}{{ {} }}\n",
            st.mangled,
            if st.is_list { "(list) " } else { "" },
            fields.join(", ")
        ));
    }
    for e in &p.enums {
        let vars: Vec<String> = e
            .variants
            .iter()
            .map(|ts: &Vec<IrType>| {
                let inner: Vec<String> = ts.iter().map(|t| format!("{t:?}")).collect();
                format!("({})", inner.join(", "))
            })
            .collect();
        s.push_str(&format!("enum {} {{ {} }}\n", e.mangled, vars.join(", ")));
    }
    for g in &p.strings {
        s.push_str(&format!("str @{} = {:?}\n", crc_id(&g.content), g.content));
    }
    s.push('\n');
    for f in &p.funcs {
        s.push_str(&dump_function(f));
        s.push('\n');
    }
    for (name, params, ret) in &p.exports {
        let ps: Vec<String> = params.iter().map(|(n, t)| format!("{n}: {t:?}")).collect();
        s.push_str(&format!("@export {name}({}) -> {ret:?}\n", ps.join(", ")));
    }
    if let Some(m) = &p.main_name {
        s.push_str(&format!("@main = {m}\n"));
    }
    s
}

fn crc_id(s: &str) -> u64 {
    // 安定ID（ダンプ表示用の簡易ハッシュ）
    let mut h: u64 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    h
}

pub fn dump_function(f: &IrFunction) -> String {
    let mut s = String::new();
    if f.is_decl {
        s.push_str("declare ");
    }
    let cc = if f.cconv_c { "c " } else { "" };
    let params: Vec<String> =
        f.params.iter().enumerate().map(|(i, (n, t))| format!("{n}{i}: {t:?}")).collect();
    s.push_str(&format!("fn {}[{}]({}) -> {:?}", f.name, cc, params.join(", "), f.ret));
    s.push('\n');
    if f.is_decl {
        return s;
    }
    for b in &f.blocks {
        s.push_str(&format!("  {}:\n", b.label));
        for i in &b.insts {
            s.push_str(&format!("    {}\n", dump_inst(i)));
        }
        s.push_str(&format!("    ; {}\n", dump_term(&b.term)));
    }
    s
}

fn v(i: V) -> String {
    if i == NO_V {
        "_".into()
    } else {
        format!("v{i}")
    }
}

fn vs(vs_: &[V]) -> String {
    vs_.iter().map(|x| v(*x)).collect::<Vec<_>>().join(", ")
}

fn dump_inst(i: &Inst) -> String {
    match i {
        Inst::Const { dst, val } => format!("{} = const {val:?}", v(*dst)),
        Inst::Alloca { dst, ty } => format!("{} = alloca {ty:?}", v(*dst)),
        Inst::Load { dst, addr, ty } => format!("{} = load {addr} : {ty:?}", v(*dst)),
        Inst::Store { addr, val, ty } => format!("store {val} -> {addr} : {ty:?}"),
        Inst::BinOp { dst, op, ty, a, b } => {
            format!("{} = {op:?} {a}, {b} : {ty:?}", v(*dst))
        }
        Inst::CmpOp { dst, pred, ty, a, b } => {
            format!("{} = cmp-{pred:?} {a}, {b} : {ty:?}", v(*dst))
        }
        Inst::CheckedBin { dst_val, dst_ovf, op, ty, a, b } => format!(
            "({}, {}) = checked-{op:?} {}, {} : {:?}",
            v(*dst_val),
            v(*dst_ovf),
            v(*a),
            v(*b),
            ty
        ),
        Inst::Call { dst, func, args, ret } => match dst {
            Some(d) => format!("{} = call {func}({}) : {ret:?}", v(*d), vs(args)),
            None => format!("call {func}({})", vs(args)),
        },
        Inst::FieldAddr { dst, base, struct_id, field } => {
            format!("{} = field_addr {base}, s{struct_id}::{field}", v(*dst))
        }
        Inst::ElemAddr { dst, base, index, elem } => {
            format!("{} = elem_addr {base}[{}] : {elem:?}", v(*dst), v(*index))
        }
        Inst::AddrOff { dst, base, off } => format!("{} = addroff {base} + {off}", v(*dst)),
        Inst::Cast { dst, kind, val, to } => {
            format!("{} = cast-{kind:?} {} : {to:?}", v(*dst), v(*val))
        }
        Inst::Bitcast { dst, val, to } => format!("{} = bitcast {} : {to:?}", v(*dst), v(*val)),
        Inst::CopyAgg { dst_addr, src_ptr, ty } => {
            format!("copy_agg {} <- {} : {ty:?}", v(*dst_addr), v(*src_ptr))
        }
        Inst::RcInc { val } => format!("rc_inc {}", v(*val)),
        Inst::RcDec { val } => format!("rc_dec {}", v(*val)),
        Inst::BoxVal { dst, src, ty } => format!("{} = box_val {} : {ty:?}", v(*dst), v(*src)),
    }
}

fn dump_term(t: &Term) -> String {
    match t {
        Term::Ret(None) => "ret void".into(),
        Term::Ret(Some(v_)) => format!("ret {}", v(*v_)),
        Term::Br(l) => format!("br {l}"),
        Term::CondBr(c, a, b) => format!("br {}, {a}, {b}", v(*c)),
        Term::Unreachable => "<unreachable>".into(),
    }
}
