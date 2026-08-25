# NagiScript (NGS)

LLVMベースで C/Rust/Zig/Odin と深く相互運用でき、Arduino言語並みに書きやすい文法を持つ汎用プログラミング言語です。Wasm経由でReact/TypeScriptとも連携できることを目指しています。



## 特徴

- **LLVM直マッピング**: 最適化はLLVMに丸投げする素直な設計
- **C ABI相互運用**: `extern "C"` / `export "C"` による Rust/Zig/C/Arduino との連携
- **safe / unsafe の明確な境界**: 生ポインタの参照外しは `unsafe` 内のみ。安全な領域では配列境界チェック・整数オーバーフロー検出がデフォルト
- **Zig/Odin方式ジェネリクス**: トレイト制約なしのコンパイル時ダックタイピング（モノモーフィゼーション）で高速コンパイル
- **RCベースメモリ管理**: 借用チェッカーなしの自動解放（参照カウント）
- **`.ngsx` JSX構文**: UIコンポーネントをJSX風に書ける（`.ts` に対する `.tsx` と同じ位置付け）
- **`val` / `var`**: イミュータブル変数は `val`、ミュータブル変数は `var`

## クイックスタート

### 1. インストール

```bash
# npm（推奨）
npm install -g @nagiscript/cli

# cargo（crates.io）
cargo install ngs_driver

# cargo（GitHub から直接）
cargo install --git https://github.com/TatsuyaM2667/nagiscript_lang.git ngs_driver
```

### 2. プロジェクトを作成

```bash
nagiscript init hello
cd hello
```

### 3. 実行

```bash
nagiscript run main.ngs
# => Hello from hello!
```

これで完了。エディタで `main.ngs` を開いて触ってみてください。

### ビルドコマンド一览

```bash
nagiscript check main.ngs   # 型チェックのみ
nagiscript build main.ngs   # ネイティブバイナリ生成
nagiscript run main.ngs     # ビルド＆実行
nagiscript ir main.ngs      # IR ダンプ（デバッグ用）
nagiscript wasm main.ngs    # Wasm/WAT 生成
nagiscript dts main.ngs     # TypeScript 型定義生成
```

## 実装状況

| コンポーネント | 状態 |
|---|---|
| `ngs_lexer` / `ngs_ast` / `ngs_parser` | ✅ 完成（`.ngs` / `.ngsx` 両対応） |
| `ngs_sema`（型検査・モノモーフ化・網羅性検査） | ✅ 完成 |
| `ngs_ir`（中間表現への lowering） | ✅ 完成 |
| `ngs_codegen_llvm`（NGS-IR → LLVM IR） | ✅ 実装済み（ネイティブビルド対応） |
| `ngs_codegen_wasm`（NGS-IR → Wasm/WAT + .d.ts） | ✅ 実装済み（Wasmビルド対応） |
| `ngs_std`（C ランタイム: print, alloc, Rc 等） | ✅ 実装済み |
| `nagiscript` CLI | ✅ 完成（check / ir / build / run / dts / wasm / init） |

**テスト**: 96件全てパス

### 必要ツール（ビルド時）

| ツール | 用途 | 備考 |
|---|---|---|
| `rustc` + `cargo` | コンパイラ本体のビルド | |
| `llc` | LLVM IR → オブジェクトコード | LLVM 18+ 推奨 |
| `cc`（gcc/clang） | リンク | |
| `wat2wasm`（任意） | Wasm ビルド時 | [WABT](https://github.com/WebAssembly/wabt) |
| `wasmtime`（任意） | Wasm 実行時 | |

環境変数でパスを上書きできます:

```bash
export NGS_LLC=/usr/bin/llc-18
export NGS_CC=clang-18
export NGS_WAT2WASM=/usr/local/bin/wat2wasm
```

## 使い方

### CLI コマンド

```bash
# プロジェクトの初期化
nagiscript init my-app               # ネイティブテンプレート
nagiscript init --template web app   # Web (Wasm + HTML) テンプレート
nagiscript init --template wasm w    # Wasm テンプレート

# タイプチェック
nagiscript check main.ngs

# IR ダンプ
nagiscript ir main.ngs

# ネイティブビルド & 実行
nagiscript build main.ngs -o app     # LLVM → .ll → .o → 実行バイナリ
nagiscript run main.ngs              # 一時ファイルにビルドして実行

# Wasm ビルド
nagiscript wasm main.ngs -o out      # out.wat + out.wasm 生成

# TypeScript 型定義
nagiscript dts main.ngs -o out.d.ts  # エクスポート関数の型定義を生成
```

### ライブラリとして利用する

```rust
use ngs_ir::lower::lower;

let src = r#"
fn main() { print("hello") }
"#;

// 1. パース
let file = ngs_parser::parse_source(src, "main.ngs")?;

// 2. 型検査 & モノモーフィゼーション
let typed = ngs_sema::check(&file)?;

// 3. NGS-IR への lowering
let ir = lower(&typed)?;

// 4. LLVM IR 生成
let llvm = ngs_codegen_llvm::generate(&ir, &Default::default())?;

// 5. Wasm 生成
let wat = ngs_codegen_wasm::generate_wat(&ir)?;
```

## 言語ツアー

### 基本文法

```rust
// コメントは C スタイル。セミコロンは行末で省略可能。
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

fn main() {
    val x = 10          // イミュータブル（型推論）
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
    val p = Point { x: 1.0, y: 2.0 }
    val s = Shape.Circle(3.0)
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

val p = Point.new(1.0, 2.0)   // Type.method 形式で呼ぶ
print(p.norm2())
```

### 制御フロー

```rust
// if は式（値を返せる）
val max = if a > b { a } else { b }

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
    val l = build()
    l.push(3)
    print(l.get(0))            // 境界チェック付きアクセス
    print(l.len())

    val r = Rc.new(42)         // 参照カウントポインタ（スカラーを共有）
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
    val r: Result<i32, string> = wrap(5)
    return r?                  // Err なら関数から即return
}
```

### safe / unsafe 境界

```rust
fn read_register(addr: u32) -> u32 {
    unsafe {
        val ptr = addr as *u32
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
val f = 3 as f64           // int → float
val n = big as i32         // 縮小変換
val p = addr as *u32       // 整数 → 生ポインタ
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

## プロジェクト構成

```
nagiscript_lang/
├── crates/
│   ├── ngs_lexer/          # 字句解析（.ngs / .ngsx 両対応）
│   ├── ngs_parser/         # 構文解析 → AST
│   ├── ngs_ast/            # AST / TokenKind 定義
│   ├── ngs_sema/           # 型検査・モノモーフ化・網羅性検査
│   ├── ngs_ir/             # NGS-IR（簡易SSA）+ lowering
│   ├── ngs_codegen_llvm/   # NGS-IR → LLVM IR（inkwell）
│   ├── ngs_codegen_wasm/   # NGS-IR → Wasm/WAT + TypeScript 型定義
│   ├── ngs_driver/         # CLI（check / ir / build / run / dts / wasm / init）
│   └── ngs_std/            # C ランタイム（print, alloc, Rc, List 等）
├── examples/               # サンプル .ngs スクリプト
│   ├── hello.ngs           # Hello World
│   ├── basics.ngs          # 基本文法デモ
│   ├── generics.ngs        # ジェネリクスデモ
│   ├── rc_demo.ngs         # Rc 参照カウントデモ
│   ├── list_demo.ngs       # List コレクションデモ
│   ├── unsafe_demo.ngs     # unsafe / 生ポインタデモ
│   └── react-demo/         # Wasm×React サンプル（予定）
├── npm/                    # @nagiscript/cli npm パッケージ
│   ├── package.json
│   ├── install.js          # postinstall（prebuilt ダウンロード or cargo build）
│   └── bin/nagiscript.js   # ラッパースクリプト
├── tests/
│   ├── lexer/              # 字句解析テスト（17件）
│   ├── parser/             # 構文解析テスト（25件）
│   ├── ir/                 # IR lowering テスト（18件）
│   └── driver/             # CLI 統合テスト（6件）
└── nagiscript-language-design.md   # 設計仕様書
```

## テスト

```bash
cargo test --all            # 全96件
cargo test -p ngs_lexer     # 字句解析のみ
cargo test -p ngs_parser    # 構文解析のみ
cargo test -p ngs_sema      # 型検査のみ
cargo test -p ngs_ir        # IR lowering のみ
cargo test -p ngs_driver    # CLI 統合テストのみ
```

## コンパイルパイプライン

```
[.ngs / .ngsx]
     ↓ ngs_lexer      トークン列
     ↓ ngs_parser     AST
     ↓ ngs_sema       型検査・モノモーフ化 → TypedProgram
     ↓ ngs_ir         NGS-IR（簡易SSA、LLVM非依存）
     ↓ ngs_codegen_llvm   LLVM IR → .ll → .o → リンク → 実行バイナリ
     ↓ ngs_codegen_wasm   WAT → .wasm + TypeScript 型定義
```

## ロードマップ

- [x] Stage 1: レキサ・パーサ・AST基盤
- [x] Sema（モノモーフ化ジェネリクス・safe/unsafe・網羅性）
- [x] NGS-IR lowering
- [x] LLVM codegen（ネイティブビルド）
- [x] Wasm codegen（.wasm + .d.ts 生成）
- [x] ランタイム（ngs_std: print, Rc, List, alloc 等）
- [x] CLI ドライバ（check / ir / build / run / dts / wasm / init）
- [x] テスト（96件、全パス）
- [ ] ネイティブビルドの統合テスト改善（llc / cc 依存テストのCI化）
- [ ] Wasm×React デモアプリ
- [ ] Stage 8: `.ngsx` UIコンポーネント、セルフホスト

## ライセンス

MIT
