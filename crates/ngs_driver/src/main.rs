//! nagiscript — NagiScript コンパイラドライバ。
//!
//! 使い方:
//!   nagiscript check <input.ngs>            構文解析と型チェックのみ行う
//!   nagiscript ir    <input.ngs> [-o PATH]  NGS-IR をダンプする
//!   nagiscript build <input.ngs> [-o PATH]  [--target TRIPLE] [--emit-ll]
//!                                           ネイティブ実行ファイルを生成する
//!                                           （llc + cc、ランタイムは自動リンク）
//!   nagiscript run   <input.ngs>            ビルドして即実行する
//!   nagiscript dts   <input.ngs> [-o PATH]  C エクスポートから .d.ts を生成する
//!   nagiscript wasm  <input.ngs> [-o PATH]  WAT（可能なら .wasm）を生成する
//!
//! 環境変数:
//!   NGS_CC     ネイティブリンクに使うコンパイラ（既定: cc、--target 時は clang）
//!   NGS_LLC    llc のパス（既定: PATH 上の llc）
//!   NGS_WAT2WASM wat2wasm のパス（未設定かつ不在の場合は .wat のみ出力）

use std::path::{Path, PathBuf};
use std::process::Command;

use ngs_ir::IrProgram;
use ngs_sema::TypedProgram;

const USAGE: &str = "\
nagiscript — NagiScript compiler

USAGE:
    nagiscript <COMMAND> <INPUT.ngs> [OPTIONS]

COMMANDS:
    check    parse and type-check only
    ir       dump NGS-IR
    build    produce a native executable (llc + cc)
    run      build then execute immediately
    dts      generate TypeScript declarations from C exports
    wasm     generate WAT (and .wasm when wat2wasm is available)

OPTIONS:
    -o, --output <PATH>   output path (default: derived from input name)
    --target <TRIPLE>     LLVM target triple for cross compilation
    --emit-ll             keep the intermediate .ll next to the output
    -h, --help            show this help
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = real_main(args);
    std::process::exit(code);
}

fn real_main(args: Vec<String>) -> i32 {
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprint!("{USAGE}");
        return if args.is_empty() { 2 } else { 0 };
    }
    let cmd = args[0].as_str();
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut target: Option<String> = None;
    let mut emit_ll = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                if i + 1 >= args.len() {
                    eprintln!("error: {} requires a path", args[i]);
                    return 2;
                }
                output = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--target" => {
                if i + 1 >= args.len() {
                    eprintln!("error: --target requires a triple");
                    return 2;
                }
                target = Some(args[i + 1].clone());
                i += 2;
            }
            "--emit-ll" => {
                emit_ll = true;
                i += 1;
            }
            other => {
                if other.starts_with('-') {
                    eprintln!("error: unknown option `{other}`");
                    return 2;
                }
                if input.is_some() {
                    eprintln!("error: multiple inputs given (`{}`)", input.unwrap().display());
                    return 2;
                }
                input = Some(PathBuf::from(other));
                i += 1;
            }
        }
    }
    let Some(input) = input else {
        eprintln!("error: missing <INPUT.ngs>");
        return 2;
    };
    if !["check", "ir", "build", "run", "dts", "wasm"].contains(&cmd) {
        eprintln!("error: unknown command `{cmd}`");
        return 2;
    }

    // フロントエンド（parse + sema）
    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", input.display());
            return 1;
        }
    };
    let path_str = input.to_string_lossy().to_string();
    let file = match ngs_parser::parse_source(&src, &path_str) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}: error: {e}", input.display());
            return 1;
        }
    };
    let typed = match ngs_sema::check(&file) {
        Ok(t) => t,
        Err(errs) => {
            for e in &errs {
                let pos = offset_to_linecol(&src, e.span.lo);
                eprintln!("{}:{}:{}: error: {}", input.display(), pos.0, pos.1, e.msg);
            }
            eprintln!("error: {} error(s)", errs.len());
            return 1;
        }
    };

    let ir = match ngs_ir::lower::lower(&typed) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("error: lowering failed: {e}");
            return 1;
        }
    };

    match cmd {
        "check" => 0,
        "ir" => {
            let text = ngs_ir::dump::dump_program(&ir);
            match output {
                Some(p) => {
                    if let Err(e) = std::fs::write(&p, text) {
                        eprintln!("error: cannot write {}: {e}", p.display());
                        return 1;
                    }
                    println!("{}", p.display());
                }
                None => print!("{text}"),
            }
            0
        }
        "dts" => {
            let text = ngs_codegen_wasm::generate_dts(&ir);
            match output {
                Some(p) => {
                    if let Err(e) = std::fs::write(&p, text) {
                        eprintln!("error: cannot write {}: {e}", p.display());
                        return 1;
                    }
                    println!("{}", p.display());
                }
                None => print!("{text}"),
            }
            0
        }
        "build" | "run" => {
            let temp = cmd == "run";
            let out_path = match (&output, temp) {
                (Some(p), _) => p.clone(),
                (None, true) => std::env::temp_dir()
                    .join(format!("nagiscript-run-{}", std::process::id())),
                (None, false) => default_output(&input, ""),
            };
            match build_native(&input, &ir, &out_path, target.as_deref(), emit_ll) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("error: {e}");
                    return 3;
                }
            }
            if temp {
                match Command::new(&out_path).status() {
                    Ok(st) if st.success() => 0,
                    Ok(st) => st.code().unwrap_or(1),
                    Err(e) => {
                        eprintln!("error: cannot execute {}: {e}", out_path.display());
                        3
                    }
                }
            } else {
                println!("{}", out_path.display());
                0
            }
        }
        "wasm" => {
            let base = match &output {
                Some(p) => strip_ext(p),
                None => default_output(&input, ""),
            };
            let wat_path = with_ext(&base, "wat");
            let wat = match ngs_codegen_wasm::generate_wat(&ir) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("error: wasm codegen failed: {e}");
                    return 1;
                }
            };
            if let Err(e) = std::fs::write(&wat_path, &wat) {
                eprintln!("error: cannot write {}: {e}", wat_path.display());
                return 1;
            }
            // d.ts を併せて出力（エクスポートがある場合）
            if !ir.exports.is_empty() {
                let dts_path = with_ext(&base, "d.ts");
                let _ = std::fs::write(&dts_path, ngs_codegen_wasm::generate_dts(&ir));
            }
            let tool = std::env::var("NGS_WAT2WASM").unwrap_or_else(|_| "wat2wasm".into());
            match Command::new(&tool).args([wat_path.to_str().unwrap_or(""), "-o"]).output() {
                Ok(_) => {
                    // -o だけ渡しても出力先が無いので素直に再実行する
                    let wasm_path = with_ext(&base, "wasm");
                    match Command::new(&tool)
                        .args([wat_path.to_str().unwrap_or(""), "-o", wasm_path.to_str().unwrap_or("")])
                        .output()
                    {
                        Ok(o) if o.status.success() => {
                            println!("{}", wasm_path.display());
                            0
                        }
                        Ok(o) => {
                            eprint!("{}", String::from_utf8_lossy(&o.stderr));
                            eprintln!("error: {tool} rejected the generated WAT");
                            3
                        }
                        Err(e) => {
                            eprintln!("warning: cannot run {tool}: {e} (.wat is kept)");
                            println!("{}", wat_path.display());
                            0
                        }
                    }
                }
                Err(_) => {
                    println!("{}", wat_path.display());
                    0
                }
            }
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// ネイティブビルド（addendum Stage 9: --target によるクロスビルド対応）
// ---------------------------------------------------------------------------

fn build_native(
    _input: &Path,
    ir: &IrProgram,
    out: &Path,
    target: Option<&str>,
    emit_ll: bool,
) -> Result<(), String> {
    if let Some(p) = out.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = out.parent().unwrap_or(Path::new(".")).to_path_buf();

    // 1) LLVM IR 生成
    let opts = ngs_codegen_llvm::LlvmOptions {
        gen_main_wrapper: true,
        target_triple: target.map(|t| t.to_string()),
    };
    let ll = ngs_codegen_llvm::generate(ir, &opts).map_err(|e| format!("llvm codegen: {e}"))?;
    let ll_path = dir.join(format!("{stem}.ll"));
    std::fs::write(&ll_path, &ll).map_err(|e| format!("write {}: {e}", ll_path.display()))?;

    // 2) オブジェクト化（llc があればそれを使い、なければ clang に直読みさせる）
    let obj_path = dir.join(format!("{stem}.o"));
    let llc = std::env::var("NGS_LLC").unwrap_or_else(|_| "llc".into());
    let cc = match std::env::var("NGS_CC") {
        Ok(c) => c,
        Err(_) => if target.is_some() { "clang".into() } else { "cc".into() },
    };
    let mut llc_args = vec![ll_path.to_string_lossy().to_string(), "-filetype=obj".into()];
    if let Some(t) = target {
        llc_args.push(format!("-mtriple={t}"));
    }
    llc_args.push("-o".into());
    llc_args.push(obj_path.to_string_lossy().to_string());
    let ok = Command::new(&llc).args(&llc_args).output();
    match ok {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            // llc が失敗したら clang で直接 .ll をコンパイルしてみる
            let mut cargs = vec!["-c".into(), ll_path.to_string_lossy().to_string(), "-o".into()];
            if let Some(t) = target {
                cargs.insert(0, format!("--target={t}"));
            }
            cargs.push(obj_path.to_string_lossy().to_string());
            let co = Command::new(&cc)
                .args(&cargs)
                .output()
                .map_err(|e| format!("cannot run {cc}: {e}"))?;
            if !co.status.success() {
                return Err(format!(
                    "code generation failed:\nllc:\n{}\nclang:\n{}",
                    String::from_utf8_lossy(&o.stderr),
                    String::from_utf8_lossy(&co.stderr)
                ));
            }
        }
        Err(e) => return Err(format!("cannot run {llc}: {e}")),
    }

    // 3) ランタイムとリンク
    let rt_path = dir.join(format!("{stem}_runtime.c"));
    std::fs::write(&rt_path, ngs_std::RUNTIME_C).map_err(|e| format!("write runtime: {e}"))?;
    let mut largs: Vec<String> = vec![];
    if let Some(t) = target {
        largs.push(format!("--target={t}"));
    }
    largs.push(obj_path.to_string_lossy().to_string());
    largs.push(rt_path.to_string_lossy().to_string());
    // 文字列定数への絶対参照を含むため PIE を無効化する
    largs.push("-no-pie".into());
    largs.push("-lm".into());
    largs.push("-o".into());
    largs.push(out.to_string_lossy().to_string());
    let lo = Command::new(&cc)
        .args(&largs)
        .output()
        .map_err(|e| format!("cannot run {cc}: {e}"))?;
    if !lo.status.success() {
        return Err(format!("link failed:\n{}", String::from_utf8_lossy(&lo.stderr)));
    }
    let _ = std::fs::remove_file(&rt_path);
    let _ = std::fs::remove_file(&obj_path);
    if !emit_ll {
        let _ = std::fs::remove_file(&ll_path);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// パス・診断ユーティリティ
// ---------------------------------------------------------------------------

fn default_output(input: &Path, ext: &str) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "out".into());
    let mut name = stem;
    if !ext.is_empty() {
        name.push('.');
        name.push_str(ext);
    }
    input.parent().unwrap_or(Path::new(".")).join(name)
}

fn strip_ext(p: &Path) -> PathBuf {
    let mut pb = p.to_path_buf();
    pb.set_extension("");
    pb
}

fn with_ext(base: &Path, ext: &str) -> PathBuf {
    let mut s = base.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// バイトオフセット → (行, 列)（1 起点）
fn offset_to_linecol(src: &str, off: usize) -> (usize, usize) {
    let off = off.min(src.len());
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
