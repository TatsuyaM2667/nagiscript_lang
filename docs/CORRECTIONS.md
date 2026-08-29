# NagiScript ドキュメント修正メモ（現行コンパイラーとの差異）

> この文書は `docs/` にあるドキュメント群と、**現行の NagiScript コンパイラーが
> 実際にサポートする機能**の差異をまとめたものです。
>
> `docs/` の多くは将来の仕様案（ロードマップ）を記述しており、**現行の
> `nagiscript` コマンドでは動きません**。本ファイルを現行仕様の「正」とし、
> 各ページに修正を反映してください。

**確認日:** 2026-08-28（コンパイラー v0.2.0）

---

## 1. 用語: 基本的なこと（全ページ共通）

### 1.1 型名は `str` ではなく `string`

`docs/` では `str` と書いていますが、**現行の型名は `string`** です。

```ngs
// 動く
fn greet(name: string) -> string {
    return name
}

// 動かない
// fn greet(name: str) -> str { return name }   // unknown type `str`
```

### 1.2 関数の戻り値

- 戻り値の型は `fn name(...) -> 型` の **`->` 構文**です。
  `docs/` にある `fn add(a: i32, b: i32) i32 { ... }`（`->` なし）は**動きません**。
- ブロック末尾の暗黙 return（式ブロック）は**サポートされていません**。
  必ず `return` を明示してください。

```ngs
// 動く
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

// 動かない
// fn add(a: i32, b: i32) -> i32 { a + b }
```

### 1.3 キーワードの実在

現行のキーワード / 組み込みは以下だけです。**これ以外は識別子として扱われ、
`expected item` 等のエラーになります。**

```
fn  val  var  if  else  for  while  match
struct  enum  impl  unsafe  extern  export  return
in  as  break  continue  true  false
print  println  panic
```

**存在しない（docs にだけ登場する）もの:**
`import` / `export((文))` / `io.*` / `std:*` / `net.*` / `loop` / `async` / `await`
`and` / `or` / `not`（論理キーワード） / `void` 戻り / `null` / `type` / `const`

> 論理演算は `&&` `||` `!` を使用します（キーワード `and/or/not` は使えません）。

```ngs
// 動く
val a = true
val b = true
if a && b {
    print("ok")
}
```

---

## 2. モジュール / import（docs/tutorial/functions.md, getting-started.md 等）

**`import` は未実装です。** 複数ファイル分割はできません。

- 修正: 「モジュール / import」の章は**ロードマップ（未実装）**として書き直す。
- `export` は関数宣言の前置のみ可能（Wasm/C 向け）。「export 文」で変数を出すのは不可。

---

## 3. 出力: `io.println` → `print` / `println`

`docs/` の `io.println("...")` は**動きません**。

- 組み込みは `print(値)` と `println(値)` です。
- 引数は**ちょうど 1 個**。文字列と数値、bool を表示できます。
- 文字列を複数つなげたい `io.println("a" + "b")` は**不可能**（文字列の `+` は未実装）。
  文字列連結はできません。

```ngs
fn main() {
    println("Hello, World!")   // 改行付き
    print("no newline")
    print(42)
}
```

**重要: 文字列の `+` 連結は不可。** `docs/reference/operators.md` の「文字列演算子 +」は
誤り。`"Hello" + " World"` はコンパイルエラーになります。

### 3.1 文字列補間 `f"..."`（実装済み）

文字列リテラルを `f"..."` にすると、`{式}` が評価され出力に埋め込まれます。
**print / println の引数でのみ有効**です（それ以外の式で使うと
「f-string is only valid directly inside print/println」エラー）。

```ngs
fn main() {
    var x = 42
    var pi = 3.5
    var name = "world"
    println(f"hello {name}, x={x}, pi={pi}, ok={true}")  // hello world, x=42, ...
    println(f"{x} + 8 = {x + 8}")                         // 式も可
    println(f"esc \{x\} = {1}")                           // \{ \} でリテラルな波括弧
}
```

- 埋め込めるのは数値・bool・string です（それ以外の型は
  `cannot interpolate ...` エラー）。
- リテラルな `{` / `}` を表示するには `\{` / `\}` とエスケープします
  （Python の `{{ }}` は非対応）。
- 実装方式は「print-time 展開」: `f"a{x}b"` は `__ngs_print_str("a")` /
  `__ngs_print_*(x)` / `__ngs_print_str("b")` の一連の print 呼び出しに変換されます。

---

## 4. 制御構文

### 4.1 `for` ループ（旧バグは修正済み）

以前は `for i in 0..5 { print(i) }` のループ本体が IR から落ちて何も出力されない
バグがありましたが、**修正済み**です（sema の void 末尾式を文として扱う修正）。`while` と
同等に使えます。

- `docs/` の `for i in 0..10 step 2` 構文の `step` は**未実装**。

### 4.2 `loop` は存在しない

`docs/` の `loop { }` は**使えません**。`while true { }` とします。

### 4.3 `match`

- パターンは**裸の列挙バリアント名**（`Ok`, `NotFound`）で、`_` のワイルドカードを
  使用します（docs の `Shape.Circle(r)` 形式は可、`HttpStatus.Ok` を**値として**
  生成するときは `HttpStatus.Ok()` と括弧が必要）。

```ngs
enum Status { Ok NotFound }

fn show(s: Status) {
    val st = match s {
        Ok => "200",
        _ => "404",
    }
    println(st)
}
```

### 4.4 `if` は式ではない（docs の式としての if は不正確）

`if` は**文**として使うのが確実です。

---

## 5. 構造体 / 列挙型 / impl

### 5.1 構造体リテラル

```ngs
struct Point { x: f32 y: f32 }
val p = Point { x: 3.0, y: 4.0 }
println(p.x)
```

### 5.2 メソッドは `impl` ブロック。`self` は型注釈必須・**参照渡し**

`docs/` の `fn Point.distance_to(self: Point, ...)` 形式（`Point.` 前置）は**動きません**。
`impl` ブロックを使い、`self` に型注釈が必要です。

- **`self` は暗黙に参照として渡されます**（値コピーではない）。メソッド内で
  `self.field = ...` とフィールドを変更すると呼び出し元のインスタンスに反映されます。
- 構造体・enum の**フィールド代入**（`s.n = v` / `s.n += v`）と、メソッドでの
  `self` 経由のフィールドミューテーションは**実装済み・動作確認済み**。

```ngs
struct Point { x: f32 y: f32 }

impl Point {
    fn show(self: Point) -> string {
        return "point"
    }
    fn move_by(self: Point, dx: f32, dy: f32) {
        self.x = self.x + dx   // 呼び出し元に反映される
        self.y = self.y + dy
    }
}

fn main() {
    val p = Point { x: 1.0, y: 2.0 }
    println(p.show())
    p.move_by(3.0, 4.0)      // self 参照渡しで p が更新される
}
```

### 5.3 enum の値生成は括弧

`docs/` の `Color.Red`（括弧なし）は動かない場合があります。単一の値として使うときは
`Color.Red()`。

```ngs
enum Color { Red Green }
val c = Color.Red()   // 値の生成（ユニットバリアント）
```

### 5.4 関数型フィールド / 高階関数は非推奨

`docs/` の `handler: fn(Request) Response` のようなフィールドや、
`fn apply(f: fn(i32) i32, ...)` 等の関数型は、現行では**サポートが不安定**。
ルーターは `handler_id: i32` と `if` 分岐で実装すること（本リポジトリの
`/home/tatsuya/web_framework/main.ngs` 参照）。

---

## 6. 型システム（docs/reference/types.md）

### 6.1 実際に使える型

```
void(bool の単位), bool, string, i8 i16 i32 i64, u8 u16 u32 u64, usize isize,
f32 f64, *T(ポインタ), [T; N](固定配列), 構造体, enum, Rc<T>, Gen<T> 等なし
```

- `str` → `string`
- `void`（単型としての）は無い。戻りなし関数は `->` を書かない。

```ngs
fn do_nothing() {
    // 戻りなし
}
```

### 6.2 タプル・ジェネリクス構造体の記法

- docs の `Vec2<T>` ジェネリクス構造体・`(i32, str)` タプルは**未実装/未検証**。
- 実用法: `struct Vec2 { x: f32 y: f32 }`。型パラメータは `List<i32>` 以外は限定的。

### 6.3 List / Rc

- `List<T>`: `List.new()` / `push` / `len` / `get` / `set` は利用可。
  **旧来の `zext i64 to i64` によるリンク失敗バグは修正済み**（同一 LLVM 型へのキャストを
  スキップする修正）。`examples/list_demo.ngs`（`print(l.len())` / `get` / `set`）が動作確認済み。
- `Rc`: `Rc.new(値)` / `.get()` はあるが、`.get()` の戻り値が意図と異なる
  （例: `Rc.new(42).get()` が `1` を返す）。**未修正の既知バグ。当面は構造体 + 値渡しを推奨。**

### 6.4 数値の縮小変換は `as` 必須（A4）

`i64` → `i32` 等、ビット幅が縮む整数変換は**暗黙不可**とし、明示的な `as` を要求します
（オーバーフローでデータが失われるため）。

```ngs
val big: i64 = 5000000000
val small: i32 = big          // エラー: narrowing conversion requires `as`
val small2: i32 = big as i32  // OK
```

---

## 7. ポインタ / unsafe

`docs/` の `*const i32` / `*mut i32` / `&&`、`?` 演算子:

- 実用可能: `*i32` 形式、**単一 `&x as *i32`**、`unsafe { *ptr }`（`examples/unsafe_demo.ngs`）。
- **アドレス取得は単一 `&`** に統一（A5）。旧 `&&x` は論理AND `&&` と衝突するため不採用。
  `&x as *i32` と書きます。
- `docs/` の `ptr->` / `ptr.offset(n)` は**未検証**。`*ptr` と整数演算で代替。

```ngs
fn read(ptr: *i32) -> i32 {
    unsafe { return *ptr }
}
var x = 42
val p = &x as *i32
println(read(p))
```

- **`?` 演算子（エラー伝播）は未実装。** Result 型も組み込みとしては存在せず、
  enum で自作する。

---

## 8. 非同期（docs/tutorial/async.md）

`async` / `await` は**完全に未実装**。このページはロードマップ扱いに。

---

## 9. WebAssembly / JSX

- Wasm 出力は `nagiscript wasm main.ngs [-o PATH]` でサポート（`.wat` / 可能なら
  `.wasm` + `.d.ts`）。**ネイティブビルドとの挙動違いに注意。**
- `export "C" fn` 形式が Wasm / C エクスポートの正しい書き方
  （`docs/` の `export fn` は誤り）。

```ngs
export "C" fn add(a: i32, b: i32) -> i32 {
    return a + b
}
```

- JSX（`.ngsx`）はランタイムに props 基盤が存在するが、**ドキュメント化は保留**。

---

## 10. 標準ライブラリ（docs/reference/standard-library.md 等）

**「std:io」「std:math」「std:fs」「std:args」「std:async」「std:net」等の
モジュールは存在しません。** これらは全部ロードマップ扱い。

- 現行の標準的な API は `print` / `println` / `panic` と、ランタイムの
  `__ngs_*`（List / Rc / 文字列比較・数値変換）。
- C 標準関数は `extern "C" fn` で宣言して利用可能。

```ngs
extern "C" fn strlen(s: *u8) -> i32
```

---

## 11. キーワード一覧（docs/reference/keywords.md）

上記 1.3 のリストに修正してください。以下は**存在しません**:
`import` `export(文)` `async` `await` `loop` `type` `const` `class` `interface`
`trait` `impl`(は存在・impl ブロック用) `pub` `self`(変数名; 型注釈は要る) `step` `and` `or` `not`

---

## 12. 動作確認済みの最小テンプレート（現行で動くもの）

```ngs
enum Status { Ok NotFound }

struct Response {
    status: Status
    body: string
}

fn ok_res() -> Response {
    return Response { status: Status.Ok(), body: "OK" }
}

fn main() {
    val r = ok_res()
    val st = match r.status {
        Ok => "200",
        _ => "404",
    }
    println(st)
    println(r.body)
    println("done 42")
}
```

---

## 13. 修正優先度まとめ

| 優先 | docs ページ | 主な修正内容 |
|------|-------------|--------------|
| ★★★ | getting-started.md, tutorial/functions.md | `import` `io.*` → 未実装扱い / `print` / `->` / `string` |
| ★★★ | tutorial/basics.md | `val/var` は〇、`str`→`string`、`for` バグ注記、`step` 削除 |
| ★★★ | reference/syntax.md, operators.md | `==` 等は〇、`+` 文字列連結・`?`・`and/or/not`・`->` を修正 |
| ★★ | reference/types.md, keywords.md | 型名・キーワード表の修正 |
| ★★ | reference/standard-library*.md | std: モジュール → ロードマップ、C は `extern "C"` |
| ★★ | reference/compiler.md | アーキテクチャは〇、用語（str→string 等）修正 |
| ★ | tutorial/error-handling.md | Result / `?` → enum 自作の例に差し替え |
| ★ | tutorial/async.md, memory.md, webassembly.md | async 削除、List/Rc バグ注記、export "C" |
| ★ | tutorial/microcontroller.md, cli.md, cinterop.md | ロードマップ明記、`extern "C"` 形式に |
| ★ | examples/*.md | 全コードを現行構文に差し替え（todo/web/iot） |

---

*本ファイルは docs 修正の一次情報。現行のコンパイラー挙動が変わったら随時更新してください。*
