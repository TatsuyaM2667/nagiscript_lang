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

#[test]
fn null_is_allowed_in_unsafe_only() {
    // unsafe 内では null が生ポインタ値として使える
    let p = write_tmp(
        "null_ok.ngs",
        r#"
fn main() {
    unsafe {
        val p: *u8 = null
        if (p == null) {
            println("is null")
        }
    }
}
"#,
    );
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "is null\n");

    // unsafe 外ではエラー
    let p2 = write_tmp("null_bad.ngs", "fn main() { val p: *u8 = null; }\n");
    let (code2, _so2, se2) = run(&["check", p2.to_str().unwrap()]);
    assert_eq!(code2, 1);
    assert!(se2.contains("only allowed inside an `unsafe` block"), "stderr={se2}");
}

const MATCH_VARIANTS: &str = r#"
enum Shape { Circle(i32), Point(i32, i32), Square }
fn r(s: Shape) -> i32 {
    return match s {
        Shape.Circle(r) => r
        Shape.Point(x, y) => x + y
        Shape.Square => 0
    }
}
fn main() {
    println(r(Shape.Circle(7)))
    println(r(Shape.Point(3, 40)))
    println(r(Shape.Square()))
}
"#;

#[test]
fn match_qualified_variant_bindings() {
    let p = write_tmp("m_variants.ngs", MATCH_VARIANTS);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "7\n43\n0\n");
}

const MATCH_RANGE: &str = r#"
fn grade(n: i32) -> string {
    return match n {
        90..=100 => "A"
        80..89 => "B"
        _ => "C"
    }
}
fn main() {
    println(grade(95))
    println(grade(85))
    println(grade(40))
}
"#;

#[test]
fn match_range_patterns() {
    let p = write_tmp("m_range.ngs", MATCH_RANGE);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "A\nB\nC\n");
}

const MATCH_OR_GUARD: &str = r#"
fn vowel(c: string) -> string {
    return match c {
        "a" | "e" | "i" | "o" | "u" => "vowel"
        _ => "consonant"
    }
}
fn note(v: i32) -> string {
    return match v {
        n if n >= 90 => "high"
        n if n >= 70 => "mid"
        _ => "low"
    }
}
fn main() {
    println(vowel("a"))
    println(vowel("b"))
    println(note(100))
    println(note(80))
    println(note(10))
}
"#;

#[test]
fn match_or_and_guards() {
    let p = write_tmp("m_or_guard.ngs", MATCH_OR_GUARD);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "vowel\nconsonant\nhigh\nmid\nlow\n");
}

const MATCH_NESTED: &str = r#"
enum Inner { Num(i32) }
enum Outer { Box(Inner) }
fn f(o: Outer) -> i32 {
    return match o {
        Outer.Box(Inner.Num(n)) => n + 1
    }
}
fn main() { println(f(Outer.Box(Inner.Num(41)))) }
"#;

#[test]
fn match_nested_variant_pattern() {
    let p = write_tmp("m_nested.ngs", MATCH_NESTED);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "42\n");
}

const MATCH_WILDCARD_FIELD: &str = r#"
enum Pt { Coord(i32, i32) }
fn x(p: Pt) -> i32 {
    return match p {
        Pt.Coord(a, _) => a
    }
}
fn main() { println(x(Pt.Coord(5, 9))) }
"#;

#[test]
fn match_wildcard_field() {
    let p = write_tmp("m_wild.ngs", MATCH_WILDCARD_FIELD);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "5\n");
}

const MATCH_BINDING: &str = r#"
fn id(n: i32) -> i32 {
    return match n {
        x => x
    }
}
fn main() { println(id(9)) }
"#;

#[test]
fn match_top_level_binding_covers() {
    let p = write_tmp("m_bind.ngs", MATCH_BINDING);
    let (code, so, se) = run(&["run", p.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr={se}");
    assert_eq!(so, "9\n");
}

#[test]
fn match_missing_variant_non_exhaustive() {
    let p = write_tmp("m_nonex.ngs",
        "enum Color { Red, Green, Blue }\nfn f(c: Color) -> i32 { return match c { Color.Red => 1 Color.Green => 2 } }\nfn main() {}\n");
    let (code, _so, se) = run(&["check", p.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(se.contains("missing variant(s) Blue"), "stderr={se}");
}
