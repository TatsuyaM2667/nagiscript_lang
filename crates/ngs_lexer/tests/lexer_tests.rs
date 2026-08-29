use ngs_lexer::lex;
use ngs_ast::TokenKind;

#[test]
fn lex_basic_keywords() {
    let toks = lex("fn val var if else for while match struct enum unsafe extern export return").unwrap();
    let kinds: Vec<&TokenKind> = toks.iter().map(|t| &t.kind).collect();
    assert!(kinds.contains(&&TokenKind::KwFn));
    assert!(kinds.contains(&&TokenKind::KwLet));
    assert!(kinds.contains(&&TokenKind::KwVar));
    assert!(kinds.contains(&&TokenKind::KwIf));
    assert!(kinds.contains(&&TokenKind::KwElse));
    assert!(kinds.contains(&&TokenKind::KwFor));
    assert!(kinds.contains(&&TokenKind::KwWhile));
    assert!(kinds.contains(&&TokenKind::KwMatch));
    assert!(kinds.contains(&&TokenKind::KwStruct));
    assert!(kinds.contains(&&TokenKind::KwEnum));
    assert!(kinds.contains(&&TokenKind::KwUnsafe));
    assert!(kinds.contains(&&TokenKind::KwExtern));
    assert!(kinds.contains(&&TokenKind::KwExport));
    assert!(kinds.contains(&&TokenKind::KwReturn));
}

#[test]
fn lex_integer_literals() {
    let toks = lex("0 42 100 0xFF").unwrap();
    let int_lits: Vec<u64> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::IntLit(v) => Some(*v),
        _ => None,
    }).collect();
    assert_eq!(int_lits, vec![0, 42, 100, 255]);
}

#[test]
fn lex_float_literals() {
    let toks = lex("3.14 1.0").unwrap();
    let float_lits: Vec<f64> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::FloatLit(v) => Some(*v),
        _ => None,
    }).collect();
    assert_eq!(float_lits.len(), 2);
    assert!((float_lits[0] - 3.14).abs() < f64::EPSILON);
    assert!((float_lits[1] - 1.0).abs() < f64::EPSILON);
}

#[test]
fn lex_string_literals() {
    let toks = lex(r#""hello" "world""#).unwrap();
    let str_lits: Vec<String> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::StrLit(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert_eq!(str_lits, vec!["hello", "world"]);
}

#[test]
fn lex_operators() {
    let toks = lex("+ - * / % ! == != < <= > >= && || = += -= *= /= %= -> => ? @ . ..").unwrap();
    let ops: Vec<&TokenKind> = toks.iter().filter(|t| !matches!(t.kind, TokenKind::Eof)).map(|t| &t.kind).collect();
    assert!(ops.contains(&&TokenKind::Plus));
    assert!(ops.contains(&&TokenKind::Minus));
    assert!(ops.contains(&&TokenKind::Star));
    assert!(ops.contains(&&TokenKind::Slash));
    assert!(ops.contains(&&TokenKind::Percent));
    assert!(ops.contains(&&TokenKind::Bang));
    assert!(ops.contains(&&TokenKind::EqEq));
    assert!(ops.contains(&&TokenKind::NotEq));
    assert!(ops.contains(&&TokenKind::Lt));
    assert!(ops.contains(&&TokenKind::Le));
    assert!(ops.contains(&&TokenKind::Gt));
    assert!(ops.contains(&&TokenKind::Ge));
    assert!(ops.contains(&&TokenKind::AndAnd));
    assert!(ops.contains(&&TokenKind::OrOr));
    assert!(ops.contains(&&TokenKind::Assign));
    assert!(ops.contains(&&TokenKind::PlusAssign));
    assert!(ops.contains(&&TokenKind::MinusAssign));
    assert!(ops.contains(&&TokenKind::StarAssign));
    assert!(ops.contains(&&TokenKind::SlashAssign));
    assert!(ops.contains(&&TokenKind::PercentAssign));
    assert!(ops.contains(&&TokenKind::Arrow));
    assert!(ops.contains(&&TokenKind::FatArrow));
    assert!(ops.contains(&&TokenKind::Question));
    assert!(ops.contains(&&TokenKind::At));
    assert!(ops.contains(&&TokenKind::Dot));
    assert!(ops.contains(&&TokenKind::DotDot));
}

#[test]
fn lex_punctuation() {
    let toks = lex("( ) { } [ ] , : ; |").unwrap();
    let punct: Vec<&TokenKind> = toks.iter().filter(|t| !matches!(t.kind, TokenKind::Eof)).map(|t| &t.kind).collect();
    assert!(punct.contains(&&TokenKind::LParen));
    assert!(punct.contains(&&TokenKind::RParen));
    assert!(punct.contains(&&TokenKind::LBrace));
    assert!(punct.contains(&&TokenKind::RBrace));
    assert!(punct.contains(&&TokenKind::LBracket));
    assert!(punct.contains(&&TokenKind::RBracket));
    assert!(punct.contains(&&TokenKind::Comma));
    assert!(punct.contains(&&TokenKind::Colon));
    assert!(punct.contains(&&TokenKind::Semi));
    assert!(punct.contains(&&TokenKind::Pipe));
}

#[test]
fn lex_or_or_operator() {
    let toks = lex("a || b").unwrap();
    let ops: Vec<&TokenKind> = toks.iter().filter(|t| !matches!(t.kind, TokenKind::Eof)).map(|t| &t.kind).collect();
    assert!(ops.contains(&&TokenKind::OrOr));
}

#[test]
fn lex_single_line_comment() {
    let toks = lex("x // comment\ny").unwrap();
    let idents: Vec<String> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::Ident(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert_eq!(idents, vec!["x", "y"]);
}

#[test]
fn lex_block_comment() {
    let toks = lex("x /* comment */ y").unwrap();
    let idents: Vec<String> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::Ident(s) => Some(s.clone()),
        _ => None,
    }).collect();
    assert_eq!(idents, vec!["x", "y"]);
}

#[test]
fn lex_function_declaration() {
    let toks = lex("fn add(a: i32, b: i32) -> i32 { return a + b }").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwFn);
    assert_eq!(toks[1].kind, TokenKind::Ident("add".into()));
    assert_eq!(toks[2].kind, TokenKind::LParen);
    assert_eq!(toks[3].kind, TokenKind::Ident("a".into()));
    assert_eq!(toks[4].kind, TokenKind::Colon);
}

#[test]
fn lex_struct_declaration() {
    let toks = lex("struct Point { x: f32, y: f32 }").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwStruct);
    assert_eq!(toks[1].kind, TokenKind::Ident("Point".into()));
    assert_eq!(toks[2].kind, TokenKind::LBrace);
    assert_eq!(toks[3].kind, TokenKind::Ident("x".into()));
    assert_eq!(toks[4].kind, TokenKind::Colon);
}

#[test]
fn lex_enum_declaration() {
    let toks = lex("enum Shape { Circle(f32), Rect(f32, f32) }").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwEnum);
    assert_eq!(toks[1].kind, TokenKind::Ident("Shape".into()));
    assert_eq!(toks[2].kind, TokenKind::LBrace);
    assert_eq!(toks[3].kind, TokenKind::Ident("Circle".into()));
    assert_eq!(toks[4].kind, TokenKind::LParen);
    assert_eq!(toks[5].kind, TokenKind::Ident("f32".into()));
    assert_eq!(toks[6].kind, TokenKind::RParen);
    assert_eq!(toks[7].kind, TokenKind::Comma);
}

#[test]
fn lex_for_loop() {
    let toks = lex("for i in 0..10 { print(i) }").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwFor);
    assert_eq!(toks[1].kind, TokenKind::Ident("i".into()));
    assert_eq!(toks[2].kind, TokenKind::KwIn);
    assert_eq!(toks[3].kind, TokenKind::IntLit(0));
    assert_eq!(toks[4].kind, TokenKind::DotDot);
    assert_eq!(toks[5].kind, TokenKind::IntLit(10));
}

#[test]
fn lex_match_expression() {
    let toks = lex("match x { Circle(r) => 1, _ => 0 }").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwMatch);
    assert_eq!(toks[1].kind, TokenKind::Ident("x".into()));
    assert_eq!(toks[2].kind, TokenKind::LBrace);
    assert_eq!(toks[3].kind, TokenKind::Ident("Circle".into()));
}

#[test]
fn lex_type_pointers() {
    let toks = lex("*i32 *u8 *Point").unwrap();
    let stars = toks.iter().filter(|t| matches!(t.kind, TokenKind::Star)).count();
    assert_eq!(stars, 3);
}

#[test]
fn lex_hex_literal() {
    let toks = lex("0xFF 0xAB").unwrap();
    let int_lits: Vec<u64> = toks.iter().filter_map(|t| match &t.kind {
        TokenKind::IntLit(v) => Some(*v),
        _ => None,
    }).collect();
    assert_eq!(int_lits, vec![255, 171]);
}

#[test]
fn lex_negative_numbers() {
    // Negative numbers are parsed as unary minus + integer
    let toks = lex("-42").unwrap();
    assert_eq!(toks[0].kind, TokenKind::Minus);
    assert_eq!(toks[1].kind, TokenKind::IntLit(42));
}

#[test]
fn lex_fstring_segments() {
    use ngs_ast::FStrSeg;
    let toks = lex(r#"f"a {x} b""#).unwrap();
    match &toks[0].kind {
        TokenKind::FStr(segs) => {
            assert_eq!(segs.len(), 3);
            assert!(matches!(&segs[0], FStrSeg::Text(t) if t == "a "));
            assert!(matches!(&segs[1], FStrSeg::Expr(_)));
            assert!(matches!(&segs[2], FStrSeg::Text(t) if t == " b"));
        }
        other => panic!("expected FStr token, got {other:?}"),
    }
}

#[test]
fn lex_fstring_plain_is_single_text() {
    use ngs_ast::FStrSeg;
    let toks = lex(r#"f"hello""#).unwrap();
    match &toks[0].kind {
        TokenKind::FStr(segs) => {
            assert_eq!(segs.len(), 1);
            assert!(matches!(&segs[0], FStrSeg::Text(t) if t == "hello"));
        }
        other => panic!("expected FStr token, got {other:?}"),
    }
}

#[test]
fn lex_fstring_nested_braces() {
    use ngs_ast::FStrSeg;
    let toks = lex(r#"f"{ {a: 1} } end""#).unwrap();
    match &toks[0].kind {
        TokenKind::FStr(segs) => {
            // 先頭の空テキストはflushされないため Expr("{a: 1} の内側") + Text(" end")
            assert_eq!(segs.len(), 2);
            assert!(matches!(&segs[0], FStrSeg::Expr(_)));
            assert!(matches!(&segs[1], FStrSeg::Text(t) if t == " end"));
        }
        other => panic!("expected FStr token, got {other:?}"),
    }
}

#[test]
fn lex_fstring_escaped_braces() {
    use ngs_ast::FStrSeg;
    // \{ and \} produce literal braces, not interpolation
    let toks = lex(r#"f"a \{b\} {1}""#).unwrap();
    match &toks[0].kind {
        TokenKind::FStr(segs) => {
            assert!(matches!(&segs[0], FStrSeg::Text(t) if t == "a {b} "));
            assert!(matches!(&segs[1], FStrSeg::Expr(_)));
        }
        other => panic!("expected FStr token, got {other:?}"),
    }
}

#[test]
fn lex_null_keyword() {
    let toks = lex("null").unwrap();
    assert_eq!(toks[0].kind, TokenKind::KwNull);
}

#[test]
fn lex_null_not_identifier() {
    // null は予約語なので Ident として扱われない
    let toks = lex("val x = null").unwrap();
    let mut saw_null = false;
    for t in &toks {
        if t.kind == TokenKind::KwNull {
            saw_null = true;
        }
        assert_ne!(t.kind, TokenKind::Ident("null".into()));
    }
    assert!(saw_null);
}

