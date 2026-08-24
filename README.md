# NagiScript (NGS)

LLVMベースで C/Rust/Zig/Odin と深く相互運用でき、Arduino言語並みに書きやすい文法を持つ汎用プログラミング言語です。Wasm経由でReact/TypeScriptとも連携できることを目指しています。


## 特徴

- **LLVM直マッピング**: 最適化はLLVMに丸投げする素直な設計
- **C ABI相互運用**: `extern "C"` / `export "C"` による Rust/Zig/C/Arduino との連携
- **safe / unsafe の明確な境界**: 生ポインタの参照外しは `unsafe` 内のみ。安全な領域では配列境界チェック・整数オーバーフロー検出がデフォルト
- **Zig/Odin方式ジェネリクス**: トレイト制約なしのコンパイル時ダックタイピング（モノモーフィゼーション）で高速コンパイル
- **RCベースメモリ管理**: 借用チェッカーなしの自動解放（参照カウント）
- **`.ngsx` JSX構文**: UIコンポーネントをJSX風に書ける（`.ts` に対する `.tsx` と同じ位置付け）

## 実装状況

| コンポーネント | 状態 |
|---|---|
| `ngs_lexer` / `ngs_ast` / `ngs_parser` | ✅ 完成（`.ngs` / `.ngsx` 両対応） |
| `ngs_sema`（型検査・モノモーフ化・網羅性検査） | ✅ 完成 |
| `ngs_ir`（中間表現への lowering） | ✅ 完成 |
| `ngs_codegen_llvm` | 未実装 |
| `ngs_codegen_wasm` | 未実装 |
| `ngs_std`（ランタイム） | 未実装 |
| `nagiscript` CLI | 未実装 |

**現在、実行可能バイナリはまだありません。** フロントエンド（解析〜IR生成）をライブラリとして利用できます。

## ビルドとテスト

```bash
# Rust 安定版ツールチェーンが必要です
cargo build
cargo test
```

フロントエンドの動作デモ:

```bash
cargo run -p ngs_parser --example smoke   # パース結果の表示
cargo run -p ngs_sema --example smoke     # 型検査・モノモーフ化の結果を表示
cargo test -p ngs_ir                      # lowering の統合テスト
```

## 言語ツアー

### 基本文法

```rust
// コメントは C スタイル。セミコロンは行末で省略可能。
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

fn main() {
    let x = 10          // イミュータブル（型推論）
    var y = 20          // ミュータブル
    y = y + 1
    y += add(x, y)      // 複合代入
    print(add(x, y))    // 数値 / bool / string を出力
}
```

### 構造体・enum・パターンマッチ

```rust
struct Point {
    x: f32,
    y: f32,
}

enum Shape {
    Circle(f32),
    Rect(f32, f32),
    Empty,
}

fn area(s: Shape) -> f32 {
    match s {
        // パターン側はバリアント名のみ（非修飾）
        Circle(r) => 3.14159 * r * r
        Rect(w, h) => w * h
        _ => 0.0           // スカラー型の被検査体では `_` が必須
    }
}

fn main() {
    let p = Point { x: 1.0, y: 2.0 }
    let s = Shape.Circle(3.0)
    print(area(s))
}
```

### impl ブロック（関連関数・メソッド）

```rust
impl Point {
    fn new(x: f32, y: f32) -> Point {
        return Point { x: x, y: y }
    }
    fn norm2(self: Point) -> f32 {
        return self.x * self.x + self.y * self.y
    }
}

let p = Point.new(1.0, 2.0)   // Type.method 形式で呼ぶ
print(p.norm2())
```

### 制御フロー

```rust
// if は式（値を返せる）
let max = if a > b { a } else { b }

var i = 0
while i < 10 {
    i = i + 1
    if i == 2 { continue }
    if i > 8 { break }
}

for k in 0..100 {        // レンジ for（終端は含まない）
    print(k)
}
```

### List と Rc（組込みコレクション）

```rust
fn build() -> List<i32> {
    var list = List.new()      // 要素型は注釈 or push から推論
    list.push(1)
    list.push(2)
    return list
}

fn main() {
    let l = build()
    l.push(3)
    print(l.get(0))            // 境界チェック付きアクセス
    print(l.len())

    let r = Rc.new(42)         // 参照カウントポインタ（スカラーを共有）
    print(r.get())
}
```

### Result とエラー伝播

```rust
fn wrap(v: i32) -> Result<i32, string> {
    if v > 10 {
        return Err("too big")
    }
    return Ok(v)
}

fn use_res() -> i32 {
    let r: Result<i32, string> = wrap(5)
    return r?                  // Err なら関数から即return
}
```

### safe / unsafe 境界

```rust
fn read_register(addr: u32) -> u32 {
    unsafe {
        let ptr = addr as *u32
        return *ptr            // 生ポインタの参照外しは unsafe 内のみ
    }
}
```

`unsafe` の外側では:
- 配列・List の添字アクセスは自動で境界チェック（違反時は panic）
- 整数演算はオーバーフロー検出付き
- 生ポインタの**作成**は可能だが**参照外し**は不可

### C ABI 相互運用

```rust
extern "C" fn puts(s: string);        // 外部関数の宣言

export "C" fn ngs_add(a: i32, b: i32) -> i32 {
    return a + b                       // 他言語へ公開
}
```

### キャスト

数値⇔数値、bool⇔整数、整数⇔ポインタ、ポインタ⇔ポインタが可能です。

```rust
let f = 3 as f64           // int → float
let n = big as i32         // 縮小変換
let p = addr as *u32       // 整数 → 生ポインタ
```

### `.ngsx`: JSX風UI構文

拡張子を `.ngsx` すると、JSX風マークアップ式が使えます（`createElement` 呼び出しの糖衣としてASTに展開されます）。

```jsx
// counter.ngsx
fn Counter(props: Props) -> Element {
    return (
        <div class="counter">
            <p>{props.label}</p>
            <button onClick={increment}>+1</button>
        </div>
    )
}
```

## ライブラリとして利用する

各クレートは独立しています。フロントエンド → IR の一連の流れ:

```rust
use ngs_sema::check;

let src = r#"
fn main() { print("hello") }
"#;

// 1. パース（拡張子 .ngsx で JSX モード有効化）
let file = ngs_parser::parse_source(src, "main.ngs")?;

// 2. 型検査 & モノモーフィゼーション
let typed: TypedProgram = check(&file)?;

// 3. NGS-IR への lowering
let ir: IrProgram = ngs_ir::lower(&typed)?;

// 4. （デバッグ用）IR のテキストダンプ
println!("{}", ngs_ir::dump::dump_program(&ir));
```

## プロジェクト構成

```
nagiscript_lang/
├── crates/
│   ├── ngs_lexer/          # 字句解析（.ngs / .ngsx 両対応）
│   ├── ngs_parser/         # 構文解析 → AST（JSX式を含む）
│   ├── ngs_ast/            # AST定義
│   ├── ngs_sema/           # 意味解析・型検査・モノモーフ化
│   ├── ngs_ir/             # NGS-IR（自前の中間表現）+ lowering
│   ├── ngs_codegen_llvm/   # NGS-IR → LLVM IR（inkwell）※WIP
│   ├── ngs_codegen_wasm/   # Wasmターゲット + .d.ts 生成 ※WIP
│   ├── ngs_driver/         # CLIエントリポイント ※WIP
│   └── ngs_std/            # ランタイムライブラリ ※WIP
├── examples/
│   └── react-demo/         # Wasm×React サンプル（予定）
├── tests/
│   ├── lexer/  parser/  codegen/
└── nagiscript-language-design.md   # 設計仕様書
```

## コンパイルパイプライン

```
[.ngs / .ngsx]
     ↓ ngs_lexer      トークン列
     ↓ ngs_parser     AST
     ↓ ngs_sema       型検査・モノモーフ化 → TypedProgram
     ↓ ngs_ir         NGS-IR（簡易SSA、LLVM非依存）
     ↓ ngs_codegen_*  LLVM IR / Wasm（未実装）
```

## ロードマップ

- [x] Stage 1: レキサ・パーサ・AST基盤
- [x] Sema（モノモーフ化ジェネリクス・safe/unsafe・網羅性）
- [x] NGS-IR lowering
- [ ] Stage 2: LLVM codegen（ネイティブ実行ファイル）
- [ ] Stage 4: FFI境界の本実装（ngs_std ランタイム）
- [ ] Stage 6: Wasmターゲット + React連携
- [ ] Stage 8: `.ngsx` UIコンポーネント、セルフホスト

## ライセンス

MIT
