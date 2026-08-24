fn main() {
    let src = r#"
struct Point { x: f32
    y: f32 }

enum Shape {
    Circle(f32)
    Rect(f32, f32)
}

fn area(s: Shape) -> f32 {
    match s {
        Circle(r) => 3.14159 * r * r
        Rect(w, h) => w * h
    }
}

fn add(a: i32, b: i32) -> i32 {
    return a + b
}

fn main() {
    let x = 10
    var y = 20
    y = y + 1
    print(add(x, y))
    let p = Point { x: 1.0, y: 2.0 }
    for i in 0..3 {
        print(i)
    }
    let r = if x > 5 { 1 } else { 2 }
}
"#;
    let f = ngs_parser::parse_source(src, "test.ngs").unwrap();
    println!("items: {}", f.items.len());
    for it in &f.items {
        match it {
            ngs_ast::Item::Fn(fd) => println!("fn {} params={} body={}", fd.name, fd.params.len(), fd.body.is_some()),
            ngs_ast::Item::Struct(sd) => println!("struct {} fields={}", sd.name, sd.fields.len()),
            ngs_ast::Item::Enum(ed) => println!("enum {} variants={}", ed.name, ed.variants.len()),
            ngs_ast::Item::Impl(im) => println!("impl {} methods={}", im.type_name, im.methods.len()),
        }
    }
}
