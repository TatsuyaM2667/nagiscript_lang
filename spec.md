# NagiScript (NGS) 言語仕様書 v0.4

> 本文書は NagiScript 言語の完全な仕様を網羅する。

---

## 目次

1. [��要](#1-概要)
2. [字句構造](#2-字句構造)
3. [型システム](#3-型システム)
4. [変数と定数](#4-変数と定数)
5. [関数](#5-関数)
6. [式](#6-式)
7. [文](#7-文)
8. [パターンマッチ](#8-パターンマッチ)
9. [構造体](#9-構造体)
10. [列挙型](#10-列挙型)
11. [ジェネリクス](#11-ジェネリクス)
12. [メモリモデル](#12-メモリモデル)
13. [unsafe](#13-unsafe)
14. [C ABI 相互運用](#14-c-abi-相互運用)
15. [Wasm ターゲット](#15-wasm-ターゲット)
16. [JSX / .ngsx](#16-jsx--ngsx)
17. [組込み関数と型](#17-組込み関数と型)
18. [コンパイルパイプライン](#18-コンパイルパイプライン)
19. [CLI リファレンス](#19-cli-リファレンス)
20. [制限事項と既知の問題](#20-制限事項と既知の問題)

---

## 1. 概要

NagiScript (NGS) は、LLVM をバックエンドに持ち、C/Rust/Zig/Odin と C ABI で相互運用できる汎用プログラミング言語である。

### 設計方針

| 優先度 | 方針 |
|---|---|
| 最高 | LLVM IR への直接マッピング（最適化は LLVM に丸投げ） |
| 最高 | C ABI 境界での完全な相互運用 |
| 高い | safe / unsafe の明確な分離 |
| 高い | 低い学習曲線（キーワード数最小化、1記号=1意味） |
| 高い | 軽量ランタイム、高速コンパイル |
| 中程度 | 複数バックエンド（ネイティブ / Wasm / フリースタンディング） |

### ファイル拡張子

| 拡張子 | 用途 |
|---|---|
| `.ngs` | NagiScript ソースファイル |
| `.ngsx` | JSX 構文を含む NagiScript ソースファイル |

---

## 2. 字句構造

### 2.1 キーワード

```
fn  val  var  if  else  for  while  match  struct  enum
unsafe  extern  export  return  impl  in  as  break  continue
true  false
```

**注意**: `val` はイミュータブル変数、`var` はミュータブル変数。内部表現では `val` は `KwLet` として扱われる。

### 2.2 識別子

- 先頭: 英字または `_`
- 継続: 英数字または `_`

```
foo  _bar  baz123  my_func
```

### 2.3 リテラル

#### 整数リテラル

```
0
42
100
0xFF          # 16進
0b1010        # (未対応)
```

サフィックスで型を指定可能: `42i32`, `100u64`, `255u8`。未指定時は `i32` と推論される。

#### 浮動小数点リテラル

```
3.14
1.0
2.5e10
1.5E-3
```

未指定時は `f64` と推論される。

#### 文字列リテラル

```
"hello"
"line1\nline2"
"tab\there"
"null\0char"
```

エスケープシーケンス: `\n`, `\t`, `\r`, `\0`, `\"`, `\\`。改行は含められない。

#### ブールリテラル

```
true
false
```

### 2.4 演算子

#### 二項演算子

| 演算子 | 意味 | 型制約 |
|---|---|---|
| `+` `-` `*` `/` `%` | 算術演算 | 同型同士。int+float は不可 |
| `==` `!=` `<` `<=` `>` `>=` | 比較 | 同型同士。数値/bool/str/ptr/enum |
| `&&` `\|\|` | 論理演算 | 両方 `bool` のみ |
| `=` | 代入 | 左辺は l-value |
| `+=` `-=` `*=` `/=` `%=` | 複合代入 | 同上 |
| `as` | 型キャスト | §3.8 参照 |

#### 単項演算子

| 演算子 | 意味 |
|---|---|
| `-` | 符号反転 |
| `!` | 論理否定 |
| `&&` | アドレス-of（二項 `&&` は論理 AND） |
| `*` | ポインタ参照外し（`unsafe` 内のみ） |
| `?` | `Result` エラー伝播 |

#### その他の記号

| 記号 | 意味 |
|---|---|
| `->` | 関数の返り値型 |
| `=>` | match アーム区切り |
| `..` | 範囲（`for` ループ内） |
| `\|...\|` | ラムダ関数（未実装） |
| `@` | 属性（`@repr(C)` 等） |

### 2.5 演算子の優先順位（高い順）

| 順位 | 演算子 | 結合性 |
|---|---|---|
| 1 | `.` `()` `[]` | 左結合 |
| 2 | `-`（単項）`!` `*`（デリファレンス）`&&`（アドレス） | 右結合 |
| 3 | `as` | 左結合 |
| 4 | `*` `/` `%` | 左結合 |
| 5 | `+` `-` | 左結合 |
| 6 | `<` `<=` `>` `>=` | 左結合 |
| 7 | `==` `!=` | 左結合 |
| 8 | `&&` | 左結合 |
| 9 | `\|\|` | 左結合 |
| 10 | `=` `+=` `-=` `*=` `/=` `%=` | 右結合 |

### 2.6 コメント

```
// 行コメント

/* ブロックコメント
   入れ子も可能 */
```

### 2.7 セミコロン

行末のセミコロンは省略可能。文を1行に複数書く場合はセミコロンで区切る。

```
x = 1; y = 2    // セミコロン区切り
x = 1            // 行末は暗黙の区切り
```

---

## 3. 型システム

### 3.1 組込み型

#### 整数型

| 型 | ビット幅 | 符号 |
|---|---|---|
| `i8` | 8 | あり |
| `i16` | 16 | あり |
| `i32` | 32 | あり（デフォルト整数型） |
| `i64` | 64 | あり |
| `u8` | 8 | なし |
| `u16` | 16 | なし |
| `u32` | 32 | なし |
| `u64` | 64 | なし |
| `usize` | プラットフォーム依存 | なし |
| `isize` | プラットフォーム依存 | あり |

#### 浮動小数点型

| 型 | ビット幅 |
|---|---|
| `f32` | 32 |
| `f64` | 64（デフォルト浮動小数点型） |

#### その他の基本型

| 型 | 意味 | メモリ上的サイズ |
|---|---|---|
| `bool` | 真偽値 | 1バイト |
| `str` | 文字列（immutable） | 16バイト（pointer + len） |
| `void` | ないことを示す | 0バイト |

### 3.2 ポインタ型

```
*i32       # i32 へのポインタ
*u8        # u8 へのポインタ
*Point     # 構造体へのポインタ
```

ポインタの作成は safe コードでも可能。参照外しは `unsafe` 内のみ。

### 3.3 配列型

```
[i32; 5]       # i32 の5要素固定配列
[i32; 10]      # i32 の10要素固定配列
```

配列リテラル: `[1, 2, 3]`。要素型は最初の要素から推論される。空配列は `[i32; 0]` と注釈が必要。

### 3.4 構造体型

ユーザー定義。§9 詳細。

```
struct Point {
    x: f32,
    y: f32,
}
```

### 3.5 列挙型

ユーザー定義。§10 詳細。

```
enum Shape {
    Circle(f32),
    Rect(f32, f32),
    Empty,
}
```

### 3.6 ジェネリクス型

```
List<i32>          # 組込みリスト
Result<i32, str>   # 組込み Result
Option<f64>        # 組込み Option
MyStruct<T>        # ユーザー定義ジェネリクス
```

### 3.7 型推論

- 関数の返り値型は明示的に記述する
- ローカル変数は初期値から型を推論
- 整数リテラルは `i32`、浮動小数点リテラルは `f64` と推論される
- `val x = 10` → `x: i32`
- `val y = 3.14` → `y: f64`

### 3.8 型キャスト (`as`)

| 変換元 | 変換先 | 可否 |
|---|---|---|
| 数値 → 数値 | 任意 | OK（縮小/拡大） |
| bool → 整数 | 任意 | OK |
| 整数 → bool | 任意 | OK（0 = false, それ以外 = true） |
| 整数 → ポインタ | 任意 | OK |
| ポインタ → 整数 | 任意 | OK |
| ポインタ → ポインタ | 任意 | OK |

```
val f = 3 as f64           # i32 → f64
val n = big as i32         # i64 → i32（縮小）
val p = addr as *u32       # usize → ポインタ
val b = 1 as bool          # i32 → bool（true）
```

---

## 4. 変数と定数

### 4.1 `val` — イミュータブル変数

```
val x = 10              # 型推論: i32
val name: str = "hello"  # 型注釈付き
val y: f64 = 3.14       # 明示的型指定
```

初期化後、再代入 cannot。

### 4.2 `var` — ミュータブル変数

```
var count = 0
count = count + 1        # OK
count += 1               # 複合代入も OK
```

### 4.3 型注釈

型注釈は省略可能。初期値がある場合は推論に任せる。

```
val x = 10          # 推論
val y: i64 = 10     # 明示的
var z: f32 = 1.0    # 明示的
```

---

## 5. 関数

### 5.1 関数定義

```rust
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

fn greet() {
    print("hello")        # 返り値なし（void）
}
```

- 引数には型注釈が必須
- 返り値型は `->` で指定
- `return` 文で明示的に返す、またはブロック末尾式が暗黙の返り値
- 返り値型が省略された場合は `void`

### 5.2 引数の省略

引数は全て必須。デフォルト引数や可変長引数は未実装。

### 5.3 関数呼び出し

```rust
add(1, 2)               # 基本
Point.new(1.0, 2.0)     # 関連関数（Type.method 形式）
list.push(10)           # メソッド呼び出し
```

### 5.4 関連関数（`impl` ブロック）

```rust
impl Point {
    fn new(x: f32, y: f32) -> Point {
        return Point { x: x, y: y }
    }
    fn norm2(self: Point) -> f32 {
        return self.x * self.x + self.y * self.y
    }
}

val p = Point.new(1.0, 2.0)
print(p.norm2())
```

- `Type.method` 形式で呼び出す
- `self` パラメータを持つものはメソッド（呼び出し時にレシーバーが自動渡される）
- `self` がないものは関連関数

### 5.5 ジェネリック関数

```rust
fn first<T>(list: List<T>) -> T {
    return list.get(0)
}
```

### 5.6 外部関数宣言 (`extern`)

```rust
extern "C" fn puts(s: str);
extern "C" fn sqrt(x: f64) -> f64;
```

本体がなく、セミコロンで終わる。C ABI で呼び出される。

### 5.7 エクスポート関数 (`export`)

```rust
export "C" fn ngs_add(a: i32, b: i32) -> i32 {
    return a + b
}
```

C ABI で外部に公開される。Wasm ビルド時は `export` 関数がエントリポイントになる。

### 5.8 再帰

再帰呼び出しは可能。

```rust
fn fib(n: i32) -> i32 {
    if n <= 1 { return n }
    return fib(n - 1) + fib(n - 2)
}
```

---

## 6. 式

### 6.1 リテラル式

```
42              # 整数
3.14            # 浮動小数点
true            # ブール
"hello"         # 文字列
```

### 6.2 変数参照

```
x
count
```

### 6.3 二項演算式

```
a + b
x * 2
a == b
flag && other
```

### 6.4 単項演算式

```
- x             # 符号反転
! flag          # 論理否定
&x              # アドレス取得
*p              # ポインタ参照外し（unsafe のみ）
```

### 6.5 関数呼び出し式

```
add(1, 2)
print("hello")
List.new()
```

### 6.6 フィールドアクセス式

```
p.x
self.name
```

### 6.7 配列添字式

```
arr[0]
list.get(1)     # List の場合は .get() メソッド
```

### 6.8 if 式

```rust
val max = if a > b { a } else { b }
```

`if` は式として値を返せる。`else` がなければ `void`。

### 6.9 match 式

```rust
val area = match shape {
    Circle(r) => 3.14159 * r * r,
    Rect(w, h) => w * h,
    _ => 0.0,
}
```

### 6.10 ブロック式

```rust
val x = {
    val a = 10
    val b = 20
    a + b          # ブロックの最後の式が返り値
}
```

### 6.11 unsafe ブロック式

```rust
val ptr = unsafe { addr as *u32 }
```

### 6.12 配列リテラル式

```
[1, 2, 3]
[val a, b, c + d]
```

### 6.13 構造体リテラル式

```
Point { x: 1.0, y: 2.0 }
Shape.Circle(3.0)
Shape.Rect(2, 5)
Shape.Empty
```

### 6.14 型キャスト式

```
x as f64
addr as *u32
```

### 6.15 エラー伝播式 (`?`)

```rust
fn wrap(v: i32) -> Result<i32, str> {
    if v > 10 { return Err("too big") }
    return Ok(v)
}

fn use_res() -> i32 {
    val r: Result<i32, str> = wrap(5)
    return r?          # Err なら即 return Err(...)
}
```

`expr?` は:
- `Result<T, E>` の場合: `Err(e)` なら即 `return Err(e)`、`Ok(v)` なら `v`
- `Option<T>` の場合: `None` なら即 `return None`、`Some(v)` なら `v`

### 6.16 式のネスト上限

式のネスト深度は最大 128。超えるとコンパイルエラー。

---

## 7. 文

### 7.1 変数宣言文

```
val x = 10
var y = 20
val z: f64 = 3.14
```

### 7.2 代入文

```
x = 10
y += 5
y -= 3
y *= 2
y /= 4
y %= 3
```

### 7.3 return 文

```
return
return value
return a + b
```

### 7.4 while ループ

```rust
var i = 0
while i < 10 {
    i = i + 1
    if i == 2 { continue }
    if i > 8 { break }
    print(i)
}
```

### 7.5 for ループ（レンジ）

```rust
for i in 0..10 {
    print(i)           # 0, 1, 2, ..., 9
}

for k in 0..quad(4) {  # 終端は式でも可
    print(k)
}
```

`for` はレンジのみ対応。配列のイテレーションは未対応。終端値は含まれない。

### 7.6 break / continue

```
break              # ループを抜ける
continue           # 次の反復に進む
```

### 7.7 式文

```
print("hello")     # 戻り値は無視
add(1, 2)          # 戻り値は無視
```

### 7.8 ブロック

```rust
{
    val a = 10
    val b = 20
    a + b
}
```

スコープが作られ、末尾式が暗黙の返り値。

---

## 8. パターンマッチ

### 8.1 match 文

```rust
match value {
    pattern1 => expr1,
    pattern2 => expr2,
    _ => default_expr,
}
```

### 8.2 パターンの種類

#### ワイルドカード

```
_ => 0
```

#### 整数リテラル

```
40 => 1
_ => 0
```

#### ブールリテラル

```
true => "yes"
false => "no"
```

#### 文字列リテラル

```
"hello" => 1
_ => 0
```

#### 列挙バリアント

```rust
enum Shape {
    Circle(f32),
    Rect(f32, f32),
    Empty,
}

match s {
    Circle(r) => 3.14159 * r * r,
    Rect(w, h) => w * h,
    Empty => 0.0,
    _ => 0.0,           # スカラー型では _ が必須
}
```

- バリアント名は非修飾（`Circle(r)` 而非 `Shape.Circle(r)`）
- ペイロードがある場合は括弧で束縛変数を指定
- 構造体/配列型の被検査体では `_` が必須
- 網羅性検査あり（漏れがあるとコンパイルエラー）

### 8.3 網羅性

```
val x = match v {
    1 => "one",
    _ => "other",
}
```

全ての値をカバーする必要がある。`_` を使えば網羅性を満たせる。

---

## 9. 構造体

### 9.1 定義

```rust
struct Point {
    x: f32,
    y: f32,
}

struct User {
    name: str,
    age: i32,
    active: bool,
}
```

- フィールドは `名前: 型` のペア
- フィールド間の区切りはカンマまたは改行
- 型注釈が必須

### 9.2 リテラル

```rust
val p = Point { x: 1.0, y: 2.0 }
```

フィールド名を明示する必要がある（順序指定は不可）。

### 9.3 フィールドアクセス

```
p.x
p.y
```

### 9.4 フィールド更新構文

未実装。

### 9.5 ジェネリック構造体

```rust
struct Pair<A, B> {
    first: A,
    second: B,
}
```

### 9.6 メモリレイアウト

全フィールドは8バイトアラインメントで詰められる（`align8`）。

```
struct Point {     # size=16
    x: f32,       # offset=0, size=4, padded to 8
    y: f32,       # offset=8, size=4, padded to 8
}
```

---

## 10. 列挙型

### 10.1 定義

```rust
enum Shape {
    Circle(f32),
    Rect(f32, f32),
    Empty,
}
```

- タグ付き共用体（algebraic data type）
- バリアントはペイロードを持てる
- ペイロードの型は型エクスプションで指定

### 10.2 リテラル（コンストラクタ）

```rust
val c = Shape.Circle(3.0)
val r = Shape.Rect(2.0, 5.0)
val e = Shape.Empty
```

### 10.3 メモリレイアウト

```
Enum = [tag: u64 (8B)] [payload0: 16B] [payload1: 16B]
合計: 24バイト
```

- タグは0から始まるバリアントインデックス
- ペイロードは最大2つのスロット（各16バイト）

### 10.4 組込み列挙型

#### `Result<T, E>`

```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

#### `Option<T>`

```rust
enum Option<T> {
    Some(T),
    None,
}
```

### 10.5 パターンマッチでのバリアント名

バリアント名は非修飾で使用する:

```rust
match result {
    Ok(v) => print(v),          # Ok, Err は修飾しない
    Err(e) => print(e),
}
```

---

## 11. ジェネリクス

### 11.1 基本

Zig/Odin 方式のコンパイル時ダックタイピング。トレイト制約なし。

```rust
fn first<T>(list: List<T>) -> T {
    return list.get(0)
}

struct Pair<A, B> {
    first: A,
    second: B,
}
```

### 11.2 モノモーフィゼーション

コンパイル時に具体的な型で展開される:

```
first<i32>(list)   # List<i32> 版が生成される
first<str>(list)   # List<str> 版が生成される
```

### 11.3 型推論

型パラメータは呼び出し側から推論される:

```rust
val list: List<i32> = List.new()
val x = first(list)    # T = i32 と推論
```

### 11.4 名前マングリング

```
name__TypeArg1__TypeArg2
```

例: `first__i32`, `Pair__str__i32`

---

## 12. メモリモデル

### 12.1 high-level 層（デフォルト）

- **スコープベース自動解放**: ローカル変数はスコープ離脱時に自動解放
- **RC（参照カウント）**: 共有所有に使用。循環参照は `Weak` で回避
- **静的最適化**: コンパイラが値が1箇所でのみ使用と判断できる場合、RC 操作を自動消除

### 12.2 `List<T>`

```rust
var list = List.new()      # 要素型は推論
list.push(1)
list.push(2)
print(list.get(0))         # 境界チェック付きアクセス
print(list.len())
```

内部レイアウト:

```
NgsList {
    cap: u64,       # capacity
    len: u64,       # current length
    data: *void,    # heap pointer
    esize: u64,     # element size in bytes
}
```

- 最小容量: 4
- 増長戦略: 2倍
- 境界チェック: `list.get(i)` は `i >= len` なら panic

### 12.3 `Rc<T>`

```rust
val r = Rc.new(42)         # 参照カウントポインタ
print(r.get())             # 内部値を取得
```

内部レイアウト:

```
RcHeader {
    count: u64,     # 参照カウント
    size: u64,      # ペイロードサイズ
    data: [u8; N],  # ペイロード
}
```

- `Rc.new()` はスカラー値のみ対応（構造体/配列は不可）
- 参照カウントが0になると `free`

### 12.4 `str` 型

```
NgsStrCell {
    data: *u8,      # UTF-8 データへのポインタ
    len: u64,       # バイト長
}
```

16バイト。文字列リテラルは定数領域に格納される。

### 12.5 RC 未使用時の最適化

RC 型（`List`, `Rc`）を使用しない場合、RC ランタイムコードはバイナリに含まれない（リンカのデッドコード消除 + LTO）。

### 12.6 例外メカニズム

- `panic=unwind` は未使用
- エラー伝播は `Result<T, E>` のみ
- `panic()` は即 `abort()`

---

## 13. unsafe

### 13.1 unsafe ブロック

```rust
unsafe {
    val ptr = addr as *u32
    return *ptr            # ポインタ参照外し
}
```

### 13.2 safe コードでの許可事項

- ポインタ型の作成: `val p = addr as *u32` → OK
- ポインタのアドレス取得: `val a = &x` → OK
- `as` キャスト（整数→ポインタ等）→ OK

### 13.3 unsafe のみで許可される事項

- ポインタの参照外し: `*ptr` → unsafe のみ
- 特定の外部関数呼び出し（C ランタイム関数）

### 13.4 safe コードでの制限

- 配列/List 添字アクセスは自動境界チェック（違反時 panic）
- 整数演算はオーバーフロー検出付き（違反時 panic）

---

## 14. C ABI 相互運用

### 14.1 外部関数宣言

```rust
extern "C" fn puts(s: str);
extern "C" fn sqrt(x: f64) -> f64;
extern "C" fn malloc(size: usize) -> *u8;
```

C ABI の関数を宣言。本体はなく、セミコロンで終わる。

### 14.2 関数エクスポート

```rust
export "C" fn ngs_add(a: i32, b: i32) -> i32 {
    return a + b
}
```

C ABI で外部に公開される。

### 14.3 C 互換構造体

現在は `@repr(C)` 属性は未実装。構造体はデフォルトでフィールド順に8バイトアラインで配置される。

### 14.4 C ランタイム

`ngs_std` は C 言語で実装されたランタイムを提供する:

| 関数 | 署名 | 概要 |
|---|---|---|
| `__ngs_print_str` | `(data: *u8, len: u64)` | 文字列出力 |
| `__ngs_println_str` | `(data: *u8, len: u64)` | 改行付き文字列出力 |
| `__ngs_print_i64` | `(v: i64)` | 整数出力 |
| `__ngs_print_f64` | `(v: f64)` | 浮動小数点出力 |
| `__ngs_print_bool` | `(v: i8)` | ブール出力 |
| `__ngs_panic` | `(data: *u8, len: u64)` | panic + abort |
| `__ngs_abort` | `()` | 即 abort |
| `__ngs_str_eq` | `(a: *u8, b: *u8) -> i8` | 文字列比較 |
| `__ngs_list_new` | `(esize: u64) -> *void` | リスト作成 |
| `__ngs_list_push` | `(list: *void, esize: u64) -> *void` | 要素追加 |
| `__ngs_list_len` | `(list: *void) -> u64` | 長さ取得 |
| `__ngs_list_at` | `(list: *void, idx: u64) -> *void` | 要素アクセス |
| `__ngs_list_free` | `(list: *void)` | リスト解放 |
| `__ngs_rc_new` | `(dsize: u64) -> *void` | Rc 作成 |
| `__ngs_rc_inc` | `(obj: *void)` | 参照カウント増加 |
| `__ngs_rc_dec` | `(obj: *void)` | 参照カウント減少 |

---

## 15. Wasm ターゲット

### 15.1 概要

NGS-IR → WAT → .wasm に変換。外部関数は JS 側のインポートとして扱われる。

### 15.2 型マッピング

| NGS 型 | Wasm 型 | TypeScript 型 |
|---|---|---|
| `void` | — | — |
| `bool` | `i32` | `boolean` |
| `i8` `i16` `i32` `u8` `u16` `u32` | `i32` | `number` |
| `i64` `u64` `usize` `isize` | `i64` | `number` |
| `f32` | `f32` | `number` |
| `f64` | `f64` | `number` |
| `str` `ptr` `struct` `enum` `array` | `i32` | `never`（未対応） |

### 15.3 JS インポート関数

Wasm モジュールは以下の関数を JS 側からインポートする必要がある:

```
__ngs_print_str(i32, i32)     # 文字列出力
__ngs_println_str(i32, i32)   # 改行付き文字列出力
__ngs_print_i64(i64)
__ngs_println_i64(i64)
__ngs_print_f64(f64)
__ngs_println_f64(f64)
__ngs_print_bool(i32)
__ngs_println_bool(i32)
__ngs_panic(i32, i32)
__ngs_abort()
__ngs_str_eq(i32, i32) -> i32
__ngs_box_i64(i64) -> i64
__ngs_box_f64(f64) -> i64
__ngs_box_bool(i32) -> i64
__ngs_box_str(i32) -> i64
__ngs_box_ptr(i32) -> i64
__ngs_props_new() -> i32
__ngs_props_tag(i32, i32, i32)
__ngs_props_set(i32, i32, i64, i64)
__ngs_props_add_child(i32, i64)
__ngs_fmod(f64, f64) -> f64
```

### 15.4 未対応（Wasm バックエンド）

```
__ngs_list_new      # List 操作
__ngs_list_push
__ngs_list_len
__ngs_list_at
__ngs_list_free
__ngs_rc_new        # Rc 操作
__ngs_rc_inc
__ngs_rc_dec
```

### 15.5 .d.ts 自動生成

`nagiscript dts` で TypeScript 型定義ファイルを生成:

```typescript
// out.d.ts
export function ngs_add(a: number, b: number): number;
export function greet(): void;
```

---

## 16. JSX / .ngsx

### 16.1 概要

`.ngsx` ファイルでは JSX 風のマークアップ式が使える。`createElement` 呼び出しにデサガーされる。

### 16.2 例

```jsx
// counter.ngsx
struct Props {
    label: str,
}

fn Counter(props: Props) -> Element {
    return (
        <div class="counter">
            <p>{props.label}</p>
            <button onClick={|| increment()}>+1</button>
        </div>
    )
}
```

### 16.3 内部処理

1. パーサが JSX マークアップを `ExprKind::JsxProps` にパース
2. `createElement(tag, props, children...)` の呼び出しにデサガー
3. `Props` は `NgsProps` 構造体として C ランタイムで管理

### 16.4 属性値

属性値は `BoxAny` で包まれる:

```
"string" → __ngs_box_str
123      → __ngs_box_i64
3.14     → __ngs_box_f64
true     → __ngs_box_bool
```

---

## 17. 組込み関数と型

### 17.1 組込み関数

| 関数 | 引数 | 返り値 | 概要 |
|---|---|---|---|
| `print(x)` | 数値/bool/str | void | 改行なし出力 |
| `println(x)` | 数値/bool/str | void | 改行付き出力 |
| `panic(msg)` | str | void | メッセージ付き panic + abort |
| `abort()` | なし | void | 即座に abort |
| `len(x)` | 配列/str | usize | 長さ取得 |

### 17.2 組込み型のメソッド

#### `List<T>`

| メソッド | 引数 | 返り値 | 概要 |
|---|---|---|---|
| `list.push(x)` | T | void | 要素追加 |
| `list.get(i)` | usize | T | 要素取得（境界チェック付き） |
| `list.set(i, v)` | usize, T | void | 要素設定 |
| `list.len()` | なし | usize | 長さ取得 |

#### `Rc<T>`

| メソッド | 引数 | 返り値 | 概要 |
|---|---|---|---|
| `rc.get()` | なし | T | 内部値取得 |
| `rc.value()` | なし | T | 内部値取得（`get()` のエイリアス） |

### 17.3 デフォルト型推論

| リテラル | 未注釈時のデフォルト型 |
|---|---|
| 整数 `42` | `i32` |
| 浮動小数点 `3.14` | `f64` |
| 空配列 `[]` | `[i32; 0]` |

---

## 18. コンパイルパイプライン

### 18.1 ネイティブビルド

```
[.ngs / .ngsx]
     ↓ ngs_lexer      トークン列
     ↓ ngs_parser     AST
     ↓ ngs_sema       型検査・モノモーフ化 → TypedProgram
     ↓ ngs_ir         NGS-IR（簡易SSA）
     ↓ ngs_codegen_llvm  LLVM IR
     ↓ llc            オブジェクトコード (.o)
     ↓ cc             実行バイナリ
```

### 18.2 Wasm ビルド

```
[.ngs / .ngsx]
     ↓ ngs_lexer      トークン列
     ↓ ngs_parser     AST
     ↓ ngs_sema       TypedProgram
     ↓ ngs_ir         NGS-IR
     ↓ ngs_codegen_wasm  WAT
     ↓ wat2wasm       .wasm
     + .d.ts          TypeScript 型定義
```

### 18.3 NGS-IR

中間表現は簡易SSA形式。LLVM に依存しない。

```
fn main[]() -> Void
  entry:
    v0 = const Str(0)
    v1 = addroff 0 + 0
    v2 = addroff 0 + 8
    v3 = load 1 : Ptr(U8)
    v4 = load 2 : Usize
    call __ngs_print_str(v3, v4)
    ; ret _
```

---

## 19. CLI リファレンス

### 19.1 インストール

```bash
# npm
npm install -g @nagiscript/cli

# cargo (crates.io)
cargo install ngs_driver

# cargo (GitHub)
cargo install --git https://github.com/TatsuyaM2667/nagiscript_lang.git ngs_driver
```

### 19.2 コマンド

```
nagiscript <COMMAND> [ARGS] [OPTIONS]
```

| コマンド | 使い方 | 概要 |
|---|---|---|
| `init` | `nagiscript init [NAME] [--template web\|native\|wasm]` | プロジェクト作成 |
| `check` | `nagiscript check main.ngs` | 型チェックのみ |
| `ir` | `nagiscript ir main.ngs` | NGS-IR ダンプ |
| `build` | `nagiscript build main.ngs [-o out]` | ネイティブバイナリ生成 |
| `run` | `nagiscript run main.ngs` | ビルド＆実行 |
| `wasm` | `nagiscript wasm main.ngs [-o out]` | WAT + .wasm 生成 |
| `dts` | `nagiscript dts main.ngs [-o out.d.ts]` | TypeScript 型定義生成 |

### 19.3 オプション

| オプション | 概要 |
|---|---|
| `-o, --output <PATH>` | 出力パス |
| `--target <TRIPLE>` | クロスコンパイル用 LLVM ターゲット |
| `--emit-ll` | 中間 .ll ファイルを残す |

### 19.4 環境変数

| 変数 | 概要 |
|---|---|
| `NGS_LLC` | llc のパス（デフォルト: `llc`） |
| `NGS_CC` | cc のパス（デフォルト: `cc`） |
| `NGS_WAT2WASM` | wat2wasm のパス（デフォルト: `wat2wasm`） |

### 19.5 init テンプレート

| テンプレート | 内容 |
|---|---|
| `native`（デフォルト） | ネイティブビルド用プロジェクト |
| `web` | Wasm + HTML テンプレート |
| `wasm` | Wasm のみテンプレート |

---

## 20. 制限事項と既知の問題

### 20.1 現在未対応の機能

| 機能 | 状態 |
|---|---|
| ラムダ関数 `\|\| expr` | パースのみ（Stage 8） |
| モジュールシステム / `use` | 未実装 |
| デフォルト引数 | 未実装 |
| 可変長引数 | 未実装 |
| フィールド更新構文 | 未実装 |
| `@repr(C)` 属性 | 未実装 |
| 構造体の `impl` メソッド呼び出しの改善 | 一部 |
| Wasm での List/Rc サポート | 未対応 |
| 文字列のヒープ確保（`String` 型） | 未実装 |
| `for` での配列イテレーション | 未対応（レンジのみ） |
| 2重ディスパッチ / トレイト | 未実装 |

### 20.2 既知の制限

- 整数リテラルのデフォルト型は `i32`（`u32` 等への暗黙推論は不可）
- 浮動小数点リテラルのデフォルト型は `f64`
- 文字列はヒープ確保されない（`str` は固定長スライス）
- `match` の網羅性検査は `_` パターンが必要な場合がある
- ジェネリクスは简单モノモーフィゼーションのみ（特殊化・部分特殊化は不可）
- Wasm バックエンドは List/Rc をサポートしていない

### 20.3 ロードマップ

| Stage | 内容 | 状態 |
|---|---|---|
| 0 | 計算機インタプリタ | 完了（設計段階で廃止） |
| 1 | レキサ・パーサ・AST | ✅ 完了 |
| 2 | LLVM codegen（最小） | ✅ 完了 |
| 3 | 関数・構造体・制御フロー | ✅ 完了 |
| 4 | unsafe + FFI | ✅ 実装済み |
| 5 | メモリモデル（RC） | ✅ 実装済み |
| 6 | Wasm ターゲット（Mode A） | ✅ 実装済み |
| 7 | フリースティング / 埋め込み | 未着手 |
| 8 | .ngsx Mode B + セルフホスティング | 未着手 |
| 9 | 埋め込み拡張（ESP32 / RasPi） | 未着手 |
| 10 | サーバ機能（Go 代替） | 未着手 |
| 11 | 型推論エンジン最適化 | 未着手 |
| 12 | コンテナ / Cloud-native | 未着手 |
| 13 | Linux GUI / システムユーティリティ | 未着手 |
| 14 | Kubernetes API クライアント | 未着手 |

---

## 付録: キーワード一覧

| キーワード | 意味 | v0.4 |
|---|---|---|
| `fn` | 関数定義 | |
| `val` | イミュータブル変数（内部: `KwLet`） | v0.4 で変更 |
| `var` | ミュータブル変数 | |
| `if` | 条件分岐（式でも使用可能） | |
| `else` | if の else 節 | |
| `for` | ループ（レンジのみ） | |
| `while` | ループ | |
| `match` | パターンマッチ | |
| `struct` | 構造体定義 | |
| `enum` | 列挙型定義 | |
| `unsafe` | unsafe ブロック | |
| `extern` | 外部関数宣言 | |
| `export` | 関数エクスポート | |
| `return` | 関数から返る | |
| `impl` | メソッド/関連関数ブロック | |
| `in` | for ループの range 内 | |
| `as` | 型キャスト | |
| `break` | ループ脱出 | |
| `continue` | 次の反復へ | |
| `true` | ブールリテラル | |
| `false` | ブールリテラル | |

---

## 付録: 演算子一覧

| 演算子 | 意味 | 優先度 |
|---|---|---|
| `+` `-` `*` `/` `%` | 算術 | 4-5 |
| `==` `!=` `<` `<=` `>` `>=` | 比較 | 6-7 |
| `&&` `\|\|` | 論理 | 8-9 |
| `=` `+=` `-=` `*=` `/=` `%=` | 代入 | 10 |
| `!` `-`（単項） | 否定/符号反転 | 2 |
| `&&`（単項） | アドレス取得 | 2 |
| `*`（単項） | デリファレンス | 2 |
| `?` | エラー伝播 | 1 |
| `as` | 型キャスト | 3 |
| `.` | フィールドアクセス | 1 |
| `->` | 返り値型指定 | — |
| `=>` | match アーム区切り | — |
| `..` | 範囲 | — |

---

*最終更新: 2026-08-25*
*バージョン: 0.4*
