//! NagiScript ランタイム。
//!
//! 生成コードから呼び出される `__ngs_*` 関数群のC実装 (`runtime.c`) を提供する。
//! ドライバはこのソースを埋め込んでユーザープログラムと共にコンパイルする。

/// ランタイムのCソース（driverが一時ファイルへ書き出してccに渡す）
pub const RUNTIME_C: &str = include_str!("runtime.c");

/// ランタイムが提供するシンボル一覧（デバッグ用）
pub const SYMBOLS: &[&str] = &[
    "__ngs_print_str",
    "__ngs_println_str",
    "__ngs_print_i64",
    "__ngs_println_i64",
    "__ngs_print_f64",
    "__ngs_println_f64",
    "__ngs_print_bool",
    "__ngs_println_bool",
    "__ngs_panic",
    "__ngs_abort",
    "__ngs_str_eq",
    "__ngs_str_to_i64",
    "__ngs_str_to_f64",
    "__ngs_list_new",
    "__ngs_list_push",
    "__ngs_list_len",
    "__ngs_list_at",
    "__ngs_list_free",
    "__ngs_rc_new",
    "__ngs_rc_inc",
    "__ngs_rc_dec",
    "__ngs_box_i64",
    "__ngs_box_f64",
    "__ngs_box_bool",
    "__ngs_box_str",
    "__ngs_box_ptr",
    "__ngs_props_new",
    "__ngs_props_tag",
    "__ngs_props_set",
    "__ngs_props_add_child",
    "__ngs_props_dump",
];
