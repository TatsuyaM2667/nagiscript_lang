use ngs_parser::parse_source;
use ngs_sema::check;
use ngs_ir::{lower, dump};

fn compile_to_ir(src: &str) -> ngs_ir::IrProgram {
    let file = parse_source(src, "test.ngs").unwrap();
    let typed = check(&file).unwrap();
    lower::lower(&typed).unwrap()
}

fn has_function(ir: &ngs_ir::IrProgram, name: &str) -> bool {
    ir.funcs.iter().any(|f| f.name.contains(name))
}

fn has_string(ir: &ngs_ir::IrProgram, s: &str) -> bool {
    ir.strings.iter().any(|gs| gs.content == s)
}

#[test]
fn codegen_hello_world() {
    let src = r#"
        fn main() {
            print("Hello, World!")
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
    assert!(has_string(&ir, "Hello, World!"));
}

#[test]
fn codegen_simple_function() {
    let src = r#"
        fn add(a: i32, b: i32) -> i32 {
            return a + b
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "add"));
}

#[test]
fn codegen_struct() {
    let src = r#"
        struct Point { x: i32, y: i32 }
        fn main() {
            val p = Point { x: 1, y: 2 }
            print(p.x)
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
    assert!(!ir.structs.is_empty());
}

#[test]
fn codegen_enum() {
    let src = r#"
        enum Shape { Circle(f64), Rect(i32, i32), Empty }
        fn main() {
            val s = Shape.Circle(5.0)
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
    assert!(!ir.enums.is_empty());
}

#[test]
fn codegen_if_else() {
    let src = r#"
        fn main() {
            var x = 10
            if x > 5 {
                print("big")
            } else {
                print("small")
            }
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_while_loop() {
    let src = r#"
        fn main() {
            var i = 0
            while i < 10 {
                i = i + 1
            }
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_for_range() {
    let src = r#"
        fn main() {
            for i in 0..10 {
                print(i)
            }
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_match() {
    let src = r#"
        enum Color { Red(i32), Green(i32), Blue(i32) }
        fn main() {
            val c = Color.Red(1)
            val name = match c {
                Red(v) => v,
                Green(v) => v,
                Blue(v) => v,
            }
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_list_operations() {
    let src = r#"
        fn main() {
            val l: List<i32> = List.new()
            l.push(10)
            l.push(20)
            print(l.get(0))
            print(l.len())
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_rc_operations() {
    let src = r#"
        fn main() {
            val r = Rc.new(42)
            val v = r.get()
            print(v)
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_array_literal() {
    let src = r#"
        fn main() {
            val arr = [1, 2, 3]
            print(arr[0])
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_multiple_functions() {
    let src = r#"
        fn double(x: i32) -> i32 {
            return x * 2
        }
        fn quadruple(x: i32) -> i32 {
            return double(double(x))
        }
        fn main() {
            print(quadruple(5))
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "double"));
    assert!(has_function(&ir, "quadruple"));
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_string_variable() {
    let src = r#"
        fn main() {
            val a = "Hello"
            print(a)
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
    assert!(has_string(&ir, "Hello"));
}

#[test]
fn codegen_cast() {
    let src = r#"
        fn main() {
            val x = 42
            val y = x as f64
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_dump_output() {
    let src = r#"
        fn main() {
            print("test")
        }
    "#;
    let ir = compile_to_ir(src);
    let text = dump::dump_program(&ir);
    assert!(text.contains("fn "));
    assert!(text.contains("main"));
}

#[test]
fn codegen_export_function() {
    let src = r#"
        export "C" fn exported_func() -> i32 {
            return 42
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "exported_func"));
    assert!(ir.exports.iter().any(|e| e.0 == "exported_func"));
}

#[test]
fn codegen_extern_function() {
    let src = r#"
        extern "C" fn puts(s: string);
        fn main() {
            puts("hello")
        }
    "#;
    let ir = compile_to_ir(src);
    // extern functions don't have bodies so they may not be in funcs list
    assert!(has_function(&ir, "main"));
}

#[test]
fn codegen_complex_program() {
    let src = r#"
        struct Point { x: i32, y: i32 }
        enum Shape { Circle(f64), Rect(i32, i32) }

        fn distance_sq(p: Point) -> i32 {
            return p.x * p.x + p.y * p.y
        }

        fn area(s: Shape) -> f64 {
            val result = match s {
                Circle(r) => 3.14159 * r * r,
                Rect(w, h) => (w * h) as f64,
            }
            return result
        }

        fn main() {
            val p = Point { x: 3, y: 4 }
            print(distance_sq(p))

            val s = Shape.Circle(5.0)
            print(area(s))

            var sum = 0
            for i in 0..10 {
                sum = sum + i
            }
            print(sum)
        }
    "#;
    let ir = compile_to_ir(src);
    assert!(has_function(&ir, "distance_sq"));
    assert!(has_function(&ir, "area"));
    assert!(has_function(&ir, "main"));
    assert!(!ir.structs.is_empty());
    assert!(!ir.enums.is_empty());
}
