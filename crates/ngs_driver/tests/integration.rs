//! ドライバ CLI のエンドツーエンドテスト（実バイナリを実行する）。

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_nagiscript")
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).output().expect("spawn nagiscript");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

const HELLO: &str = r#"
fn main() {
    print("hello");
    val x = 40 + 2;
    println(x);
}
"#;

fn write_tmp(name: &str, src: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("ngs_driver_tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p
}

#[test]
fn run_executes_program() {
    let p = write_tmp("hello.ngs", HELLO);
    // 実行ファイルは一時ディレクトリに置かれるため出力はプログラム自身のものだけ
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "hello42\n");
}

#[test]
fn check_reports_type_errors_with_position() {
    let p = write_tmp("bad.ngs", "fn main() { val x: i32 = \"str\"; }\n");
    let (code, _so, se) = run(&["check", p.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(se.contains("bad.ngs:1:26"), "stderr={se}");
}

#[test]
fn ir_dump_contains_functions() {
    let p = write_tmp("hello.ngs", HELLO);
    let (code, so, se) = run(&["ir", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert!(so.contains("fn "), "stdout={so}");
    assert!(so.contains("__ngs_println_i64"));
}

#[test]
fn wasm_emits_wat_and_validates() {
    let dir = std::env::temp_dir().join("ngs_driver_tests");
    let p = write_tmp("hello.ngs", HELLO);
    let out_base = dir.join("hello_wasm_out");
    let wat_path = std::path::PathBuf::from(format!("{}.wat", out_base.display()));
    let _ = std::fs::remove_file(&wat_path);
    let (code, so, se) = run(&["wasm", p.to_str().unwrap(), "-o", &format!("{}.wat", out_base.display())]);
    assert_eq!(code, 0, "stderr={se}");
    assert!(wat_path.exists(), "wat missing; stdout={so}");
    let wat = std::fs::read_to_string(&wat_path).unwrap();
    assert!(wat.contains("(module"));
    // wat2wasm がある環境では .wasm も生成され検証済みであること
    if Command::new("wat2wasm").arg("--version").output().is_ok() {
        let wasm_path = std::path::PathBuf::from(format!("{}.wasm", out_base.display()));
        assert!(wasm_path.exists(), "wasm missing");
    }
}

#[test]
fn dts_lists_exports() {
    let p = write_tmp(
        "exp.ngs",
        r#"
export "C" fn add(a: i32, b: i32) -> i32 { return a + b; }
"#,
    );
    let (code, so, se) = run(&["dts", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert!(so.contains("export function add(a: number, b: number): number;"), "stdout={so}");
}

#[test]
fn usage_errors() {
    assert_eq!(run(&[]).0, 2);
    assert_eq!(run(&["bogus"]).0, 2);
    assert_eq!(run(&["check"]).0, 2);
}

#[test]
fn fstring_interpolation_prints_segments() {
    let p = write_tmp(
        "fstr.ngs",
        r#"
fn main() {
    var x = 42;
    var pi = 3.5;
    var name = "world";
    println(f"hello {name}, x={x}, pi={pi}, ok={true}");
    println(f"{x} + 8 = {x + 8}");
    println(f"esc \{x\} = {1}");
}
"#,
    );
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(
        so,
        "hello world, x=42, pi=3.5, ok=true\n42 + 8 = 50\nesc {x} = 1\n"
    );
}

#[test]
fn fstring_outside_print_is_rejected() {
    let p = write_tmp(
        "fstr_bad.ngs",
        "fn main() { val s = f\"bad {1}\"; }\n",
    );
    let (code, _so, se) = run(&["check", p.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(se.contains("only valid directly inside print/println"), "stderr={se}");
}
