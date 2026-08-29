use ngs_parser::parse_source;
use ngs_ast::{Item, Stmt, ExprKind, UnOp, Block};

fn parse(src: &str) -> Vec<Item> {
    parse_source(src, "test.ngs").unwrap().items
}

fn get_fn_block(items: &[Item]) -> &Block {
    match &items[0] {
        Item::Fn(fd) => fd.body.as_ref().unwrap(),
        _ => panic!("expected function"),
    }
}

#[test]
fn parse_simple_function() {
    let items = parse("fn main() { print(\"hello\") }");
    assert_eq!(items.len(), 1);
    match &items[0] {
        Item::Fn(fd) => {
            assert_eq!(fd.name, "main");
            assert!(fd.params.is_empty());
            assert!(fd.body.is_some());
            let block = fd.body.as_ref().unwrap();
            assert!(block.tail.is_some());
        }
        _ => panic!("expected function declaration"),
    }
}

#[test]
fn parse_function_with_params() {
    let items = parse("fn add(a: i32, b: i32) -> i32 { return a + b }");
    match &items[0] {
        Item::Fn(fd) => {
            assert_eq!(fd.name, "add");
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].name, "a");
            assert_eq!(fd.params[1].name, "b");
            assert!(fd.ret.is_some());
        }
        _ => panic!("expected function declaration"),
    }
}

#[test]
fn parse_struct_declaration() {
    let items = parse("struct Point { x: f32, y: f32 }");
    match &items[0] {
        Item::Struct(sd) => {
            assert_eq!(sd.name, "Point");
            assert_eq!(sd.fields.len(), 2);
            assert_eq!(sd.fields[0].name, "x");
            assert_eq!(sd.fields[1].name, "y");
        }
        _ => panic!("expected struct declaration"),
    }
}

#[test]
fn parse_enum_declaration() {
    let items = parse("enum Shape { Circle(f32), Rect(f32, f32), Empty }");
    match &items[0] {
        Item::Enum(ed) => {
            assert_eq!(ed.name, "Shape");
            assert_eq!(ed.variants.len(), 3);
            assert_eq!(ed.variants[0].name, "Circle");
            assert_eq!(ed.variants[0].payload_types.len(), 1);
            assert_eq!(ed.variants[1].name, "Rect");
            assert_eq!(ed.variants[1].payload_types.len(), 2);
            assert_eq!(ed.variants[2].name, "Empty");
            assert!(ed.variants[2].payload_types.is_empty());
        }
        _ => panic!("expected enum declaration"),
    }
}

#[test]
fn parse_val_declaration() {
    let items = parse("fn f() { val x = 10 }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::Let { name, .. } => assert_eq!(name, "x"),
        _ => panic!("expected val statement"),
    }
}

#[test]
fn parse_var_declaration() {
    let items = parse("fn f() { var y = 20 }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::Let { name, mutable, .. } => {
            assert_eq!(name, "y");
            assert!(*mutable);
        }
        _ => panic!("expected var statement"),
    }
}

#[test]
fn parse_assignment() {
    let items = parse("fn f() { var x = 1; x = 2 }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 2);
    match &block.stmts[1] {
        Stmt::Assign { .. } => {}
        _ => panic!("expected assignment"),
    }
}

#[test]
fn parse_if_expression() {
    let items = parse("fn f() { val r = if true { 1 } else { 0 } }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::If { .. }));
        }
        _ => panic!("expected val with if expression"),
    }
}

#[test]
fn parse_while_loop() {
    let items = parse("fn f() { while true { break } }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::While { .. } => {}
        _ => panic!("expected while statement"),
    }
}

#[test]
fn parse_for_range_loop() {
    let items = parse("fn f() { for i in 0..10 { print(i) } }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::ForRange { var, .. } => assert_eq!(var, "i"),
        _ => panic!("expected for range"),
    }
}

#[test]
fn parse_match_expression() {
    let items = parse(r#"
        enum Shape { Circle(f32), Rect(f32, f32) }
        fn area(s: Shape) -> f32 {
            val result = match s {
                Circle(r) => r,
                Rect(w, h) => w * h,
            }
            return result
        }
    "#);
    assert_eq!(items.len(), 2);
    match &items[1] {
        Item::Fn(fd) => {
            let block = fd.body.as_ref().unwrap();
            match &block.stmts[0] {
                Stmt::Let { init, .. } => {
                    assert!(matches!(init.kind, ExprKind::Match { .. }));
                }
                _ => panic!("expected val with match expression"),
            }
        }
        _ => panic!("expected function"),
    }
}

#[test]
fn parse_struct_literal() {
    let items = parse("fn f() { val p = Point { x: 1, y: 2 } }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::StructLit { .. }));
        }
        _ => panic!("expected val with struct literal"),
    }
}

#[test]
fn parse_array_literal() {
    let items = parse("fn f() { val arr = [1, 2, 3] }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::ArrayLit(..)));
        }
        _ => panic!("expected val with array literal"),
    }
}

#[test]
fn parse_binary_operations() {
    let items = parse("fn f() { val x = 1 + 2 * 3 - 4 / 2 }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::Binary(..)));
        }
        _ => panic!("expected val with binary op"),
    }
}

#[test]
fn parse_function_call() {
    let items = parse("fn f() { print(42) }");
    let block = get_fn_block(&items);
    let tail = block.tail.as_ref().unwrap();
    assert!(matches!(tail.kind, ExprKind::Call { .. }));
}

#[test]
fn parse_method_call() {
    let items = parse("fn f() { l.push(10) }");
    let block = get_fn_block(&items);
    let tail = block.tail.as_ref().unwrap();
    assert!(matches!(tail.kind, ExprKind::Call { .. }));
}

#[test]
fn parse_unsafe_block() {
    let items = parse("fn f() { unsafe { *ptr } }");
    let block = get_fn_block(&items);
    assert_eq!(block.stmts.len(), 1);
    match &block.stmts[0] {
        Stmt::Expr(e) => {
            assert!(matches!(e.kind, ExprKind::UnsafeBlock(..)));
        }
        _ => panic!("expected unsafe expression"),
    }
}

#[test]
fn parse_cast_expression() {
    let items = parse("fn f() { val x = 1 as f64 }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::Cast(..)));
        }
        _ => panic!("expected val with cast"),
    }
}

#[test]
fn parse_extern_function() {
    let items = parse(r#"extern "C" fn puts(s: string);"#);
    match &items[0] {
        Item::Fn(fd) => {
            assert_eq!(fd.name, "puts");
            assert!(fd.extern_abi.is_some());
            assert_eq!(fd.extern_abi.as_ref().unwrap(), "C");
            assert!(fd.body.is_none());
        }
        _ => panic!("expected extern function"),
    }
}

#[test]
fn parse_export_function() {
    let items = parse(r#"export "C" fn answer() -> i32 { return 42 }"#);
    match &items[0] {
        Item::Fn(fd) => {
            assert_eq!(fd.name, "answer");
            assert!(fd.export_abi.is_some());
            assert_eq!(fd.export_abi.as_ref().unwrap(), "C");
        }
        _ => panic!("expected export function"),
    }
}

#[test]
fn parse_generics() {
    let items = parse("fn double<T>(x: T) -> T { return x }");
    match &items[0] {
        Item::Fn(fd) => {
            assert_eq!(fd.name, "double");
            assert_eq!(fd.type_params.len(), 1);
            assert_eq!(fd.type_params[0], "T");
        }
        _ => panic!("expected function with generics"),
    }
}

#[test]
fn parse_struct_generics() {
    let items = parse("struct Pair<A, B> { first: A, second: B }");
    match &items[0] {
        Item::Struct(sd) => {
            assert_eq!(sd.name, "Pair");
            assert_eq!(sd.type_params.len(), 2);
        }
        _ => panic!("expected struct with generics"),
    }
}

#[test]
fn parse_address_of() {
    let items = parse("fn f() { val p = &x }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::Unary(UnOp::AddrOf, _)));
        }
        _ => panic!("expected val with unary"),
    }
}

#[test]
fn parse_dereference() {
    let items = parse("fn f() { val v = *ptr }");
    let block = get_fn_block(&items);
    match &block.stmts[0] {
        Stmt::Let { init, .. } => {
            assert!(matches!(init.kind, ExprKind::Unary(..)));
        }
        _ => panic!("expected val with unary"),
    }
}

#[test]
fn parse_multiple_items() {
    let src = r#"
        struct Point { x: f32, y: f32 }
        enum Color { Red, Green, Blue }
        fn main() { print("hello") }
    "#;
    let items = parse(src);
    assert_eq!(items.len(), 3);
    assert!(matches!(&items[0], Item::Struct(..)));
    assert!(matches!(&items[1], Item::Enum(..)));
    assert!(matches!(&items[2], Item::Fn(..)));
}
