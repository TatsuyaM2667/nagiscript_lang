fn show(src: &str, path: &str) {
    println!("=== {path} ===");
    match ngs_parser::parse_source(src, path) {
        Err(e) => println!("parse error: {e}"),
        Ok(f) => match ngs_sema::check(&f) {
            Ok(p) => {
                for f in &p.funcs {
                    let params: Vec<String> = f.params.iter().map(|(n,t)| format!("{n}:{}", t.display())).collect();
                    println!("fn {}({}) -> {}", f.mangled, params.join(", "), f.ret.display());
                }
                for s in &p.structs {
                    let fields: Vec<String> = s.fields.iter().map(|(n,t)| format!("{n}:{}", t.display())).collect();
                    println!("struct {} {{ {} }}", s.mangled, fields.join(", "));
                }
                for e in &p.enums {
                    println!("enum {} variants={}", e.mangled, e.variants.len());
                }
            }
            Err(errs) => for e in errs { println!("error: {}", e.msg) },
        },
    }
}

fn main() {
    show(r#"
fn add(a: i32, b: i32) -> i32 { return a + b }
struct Point { x: f32, y: f32 }
enum Shape { Circle(f32), Rect(f32, f32) }
fn area(s: Shape) -> f32 {
    match s {
        Circle(r) => 3.14159 * r * r
        Rect(w, h) => w * h
    }
}
impl Point {
    fn new(x: f32, y: f32) -> Point {
        return Point { x: x, y: y }
    }
    fn norm2(self: Point) -> f32 {
        return self.x * self.x + self.y * self.y
    }
}
fn wrap(v: i32) -> Result<i32, string> {
    if v > 10 {
        return Err("too big")
    }
    return Ok(v)
}
fn use_res() -> i32 {
    let r: Result<i32,string> = wrap(5)
    return r?
}
fn build() -> List<i32> {
    var list = List.new()
    list.push(1)
    list.push(2)
    return list
}
fn main() {
    print(add(1, 2))
    let p = Point.new(1.0, 2.0)
    print(p.norm2())
    let l = build()
    print(l.len())
}
"#, "a.ngs");

    show(r#"
fn bad() {
    let p = 42 as *u32
    return *p
}
"#, "unsafe_err.ngs");
}
