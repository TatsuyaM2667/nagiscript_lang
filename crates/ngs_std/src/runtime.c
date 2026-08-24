/*
 * NagiScript ランタイム (ngs_std)
 *
 * 生成コードから呼ばれる `__ngs_*` 関数群。libc のみに依存する。
 *
 * 表現規約 (NGS-IR と一致させること):
 *   - Str の値 = {char* data; uint64_t len;} セル(16B)へのポインタ
 *   - List ヘッダ = {cap@0, len@8, data@16, esize@24} (32B)
 *   - Rc オブジェクト = {count@0, size@8, data@16..}
 *   - Box 値(JSX用) = i64 ハンドル。CrBox{tag,pad,bits} へのヒープポインタ
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

typedef struct {
    char *data;
    uint64_t len;
} NgsStrCell;

typedef struct {
    uint64_t cap;
    uint64_t len;
    void *data;
    uint64_t esize;
} NgsList;

/* ------------------------------------------------------------------ */
/* 出力                                                                */
/* ------------------------------------------------------------------ */

static void ngs_write_str(const char *data, uint64_t len) {
    fwrite(data, 1, (size_t)len, stdout);
}

void __ngs_print_str(const char *data, uint64_t len) { ngs_write_str(data, len); }

void __ngs_println_str(const char *data, uint64_t len) {
    ngs_write_str(data, len);
    fputc('\n', stdout);
}

void __ngs_print_i64(int64_t v) { printf("%lld", (long long)v); }
void __ngs_println_i64(int64_t v) { printf("%lld\n", (long long)v); }

void __ngs_print_f64(double v) {
    /* 整数値なら "3.0" のように小数部を出す */
    if (v == (double)(int64_t)v && v == v && v - v == 0 &&
        v > -9.2e18 && v < 9.2e18) {
        printf("%.1f", v);
    } else {
        printf("%g", v);
    }
}
void __ngs_println_f64(double v) { __ngs_print_f64(v); fputc('\n', stdout); }

void __ngs_print_bool(int8_t b) { fputs(b ? "true" : "false", stdout); }
void __ngs_println_bool(int8_t b) { fputs(b ? "true\n" : "false\n", stdout); }

/* ------------------------------------------------------------------ */
/* panic / abort                                                       */
/* ------------------------------------------------------------------ */

void __ngs_panic(const char *data, uint64_t len) {
    fflush(stdout);
    fputs("panic: ", stderr);
    fwrite(data, 1, (size_t)len, stderr);
    fputc('\n', stderr);
    abort();
}

void __ngs_abort(void) {
    fflush(stdout);
    fputs("abort", stderr);
    abort();
}

/* ------------------------------------------------------------------ */
/* 文字列操作                                                          */
/* ------------------------------------------------------------------ */

int8_t __ngs_str_eq(NgsStrCell *a, NgsStrCell *b) {
    if (a->len != b->len) return 0;
    if (a->len == 0) return 1;
    return memcmp(a->data, b->data, (size_t)a->len) == 0;
}

static int64_t parse_i64(const char *s, char **end) {
    return (int64_t)strtoll(s, end, 10);
}

int64_t __ngs_str_to_i64(NgsStrCell *c) {
    char buf[64];
    size_t n = c->len < sizeof(buf) - 1 ? (size_t)c->len : sizeof(buf) - 1;
    memcpy(buf, c->data, n);
    buf[n] = '\0';
    char *end = NULL;
    int64_t v = parse_i64(buf, &end);
    return v;
}

double __ngs_str_to_f64(NgsStrCell *c) {
    char buf[128];
    size_t n = c->len < sizeof(buf) - 1 ? (size_t)c->len : sizeof(buf) - 1;
    memcpy(buf, c->data, n);
    buf[n] = '\0';
    return strtod(buf, NULL);
}

/* ------------------------------------------------------------------ */
/* List<T>                                                             */
/* ------------------------------------------------------------------ */

#define LIST_MIN_CAP 4

void *__ngs_list_new(uint64_t esize) {
    NgsList *l = (NgsList *)malloc(sizeof(NgsList));
    if (!l) abort();
    l->cap = LIST_MIN_CAP;
    l->len = 0;
    l->esize = esize ? esize : 8;
    l->data = malloc((size_t)(l->cap * l->esize));
    if (!l->data) abort();
    return l;
}

void **__ngs_list_grow(NgsList *l, uint64_t need) {
    if (need <= l->cap) return NULL;
    uint64_t cap = l->cap ? l->cap * 2 : LIST_MIN_CAP;
    while (cap < need) cap *= 2;
    void *nd = realloc(l->data, (size_t)(cap * l->esize));
    if (!nd) abort();
    l->data = nd;
    l->cap = cap;
    return NULL;
}

/* 新しい要素スロットのアドレスを返す（len を +1 する） */
void *__ngs_list_push(NgsList *l, uint64_t esize) {
    (void)esize; /* ヘッダに保持している esize を優先 */
    __ngs_list_grow(l, l->len + 1);
    void *slot = (char *)l->data + (size_t)(l->len * l->esize);
    l->len += 1;
    return slot;
}

uint64_t __ngs_list_len(NgsList *l) { return l->len; }

void *__ngs_list_at(NgsList *l, uint64_t idx) {
    return (char *)l->data + (size_t)(idx * l->esize);
}

void __ngs_list_free(NgsList *l) {
    free(l->data);
    free(l);
}

/* ------------------------------------------------------------------ */
/* Rc                                                                  */
/* ------------------------------------------------------------------ */

/* レイアウト: {count@0, size@8, data@16..} */
void *__ngs_rc_new(uint64_t dsize) {
    uint8_t *obj = (uint8_t *)malloc(sizeof(uint64_t) * 2 + (size_t)dsize);
    if (!obj) abort();
    *(uint64_t *)(obj) = 1;          /* count */
    *(uint64_t *)(obj + 8) = dsize;  /* size */
    return obj;
}

void __ngs_rc_inc(void *obj) {
    if (!obj) return;
    uint64_t *count = (uint64_t *)obj;
    *count += 1;
}

void __ngs_rc_dec(void *obj) {
    if (!obj) return;
    uint64_t *count = (uint64_t *)obj;
    if (*count <= 1) {
        free(obj);
    } else {
        *count -= 1;
    }
}

/* ------------------------------------------------------------------ */
/* JSX 用ボックス化 (CrValue)                                           */
/* ------------------------------------------------------------------ */

enum {
    NGS_BOX_NONE = 0,
    NGS_BOX_I64 = 1,
    NGS_BOX_F64 = 2,
    NGS_BOX_BOOL = 3,
    NGS_BOX_STR = 4,
    NGS_BOX_PTR = 5,
};

typedef struct {
    uint32_t tag;
    uint32_t pad;
    union {
        int64_t i;
        double f;
        struct {
            const char *data;
            uint64_t len;
        } s;
        void *p;
    } bits;
} NgsBox;

static int64_t box_new(uint32_t tag) {
    NgsBox *b = (NgsBox *)malloc(sizeof(NgsBox));
    if (!b) abort();
    b->tag = tag;
    b->pad = 0;
    memset(&b->bits, 0, sizeof(b->bits));
    return (int64_t)(intptr_t)b;
}

int64_t __ngs_box_i64(int64_t v) {
    int64_t h = box_new(NGS_BOX_I64);
    ((NgsBox *)(intptr_t)h)->bits.i = v;
    return h;
}

int64_t __ngs_box_f64(double v) {
    int64_t h = box_new(NGS_BOX_F64);
    ((NgsBox *)(intptr_t)h)->bits.f = v;
    return h;
}

int64_t __ngs_box_bool(int8_t v) {
    int64_t h = box_new(NGS_BOX_BOOL);
    ((NgsBox *)(intptr_t)h)->bits.i = v ? 1 : 0;
    return h;
}

int64_t __ngs_box_str(NgsStrCell *cell) {
    int64_t h = box_new(NGS_BOX_STR);
    NgsBox *b = (NgsBox *)(intptr_t)h;
    b->bits.s.data = cell->data;
    b->bits.s.len = cell->len;
    return h;
}

int64_t __ngs_box_ptr(void *p) {
    int64_t h = box_new(NGS_BOX_PTR);
    ((NgsBox *)(intptr_t)h)->bits.p = p;
    return h;
}

/* ------------------------------------------------------------------ */
/* JSX props オブジェクト                                              */
/* ------------------------------------------------------------------ */

typedef struct NgsProp {
    struct NgsProp *next;
    char *name;
    NgsBox value;
} NgsProp;

typedef struct NgsProps {
    NgsProp *head;
    char *tag;
    NgsBox *children;
    uint64_t nchild;
    uint64_t child_cap;
} NgsProps;

void *__ngs_props_new(void) {
    NgsProps *p = (NgsProps *)calloc(1, sizeof(NgsProps));
    if (!p) abort();
    return p;
}

void __ngs_props_tag(NgsProps *p, const char *data, uint64_t len) {
    p->tag = (char *)malloc((size_t)len + 1);
    if (!p->tag) abort();
    memcpy(p->tag, data, (size_t)len);
    p->tag[len] = '\0';
}

void __ngs_props_set(NgsProps *p, const char *name, uint64_t namelen, int64_t boxed) {
    NgsProp *prop = (NgsProp *)malloc(sizeof(NgsProp));
    if (!prop) abort();
    prop->name = (char *)malloc((size_t)namelen + 1);
    if (!prop->name) abort();
    memcpy(prop->name, name, (size_t)namelen);
    prop->name[namelen] = '\0';
    prop->value = *(NgsBox *)(intptr_t)boxed;
    free((void *)(intptr_t)boxed);
    prop->next = p->head;
    p->head = prop;
}

void __ngs_props_add_child(NgsProps *p, int64_t boxed) {
    if (p->nchild == p->child_cap) {
        uint64_t cap = p->child_cap ? p->child_cap * 2 : 4;
        NgsBox *nc = (NgsBox *)realloc(p->children, (size_t)(cap * sizeof(NgsBox)));
        if (!nc) abort();
        p->children = nc;
        p->child_cap = cap;
    }
    p->children[p->nchild] = *(NgsBox *)(intptr_t)boxed;
    free((void *)(intptr_t)boxed);
    p->nchild += 1;
}

/* デバッグ/ホスト連携用: props を標準出力へダンプ */
void __ngs_props_dump(NgsProps *p) {
    printf("<%s", p->tag ? p->tag : "?");
    for (NgsProp *pr = p->head; pr; pr = pr->next) {
        switch (pr->value.tag) {
        case NGS_BOX_I64: printf(" %s=%lld", pr->name, (long long)pr->value.bits.i); break;
        case NGS_BOX_F64: printf(" %s=%g", pr->name, pr->value.bits.f); break;
        case NGS_BOX_BOOL: printf(" %s=%s", pr->name, pr->value.bits.i ? "true" : "false"); break;
        case NGS_BOX_STR: printf(" %s=\"%.*s\"", pr->name, (int)pr->value.bits.s.len, pr->value.bits.s.data); break;
        default: printf(" %s=<ptr>", pr->name); break;
        }
    }
    printf(">(%llu children)", (unsigned long long)p->nchild);
}
