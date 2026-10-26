//! Who may read the document's relations, and so ownership.
//!
//! What a float, note or follow relation makes of a block reached composition
//! three times by a read nothing keyed (B3 fixes, rounds 2, 3 and 4). Rust
//! cannot say that a module may not call `Document::relations`, so this does:
//! it lists, by file and function, the places in `crates/layout/src` that read
//! relations, and fails for a place that is not on the list. Composition reads them
//! through `ownership.rs` alone (`Ownership::class`), which the flow of a
//! running story answers from its digest and the body's from one read when it
//! begins; neither the flow nor the table, running or inline code may appear
//! here. A new read that has to be here (a pass over the relations, the key of
//! a memo) is added with the reason it is not composition.

use std::path::{Path, PathBuf};

/// Exact production functions allowed to read relations, with their reason.
/// An exemption never extends to another function in the same file or impl.
const READERS: &[(&str, &str, &str)] = &[
    (
        "incremental.rs",
        "PassKey::new",
        "region/final-pass memo inputs",
    ),
    ("incremental.rs", "LayoutJob::partial", "settled-state flag"),
    (
        "incremental.rs",
        "LayoutJob::record_graph",
        "dependency graph after evaluation",
    ),
    (
        "ownership.rs",
        "Classes::read",
        "the keyed composition classification door",
    ),
    ("regions.rs", "active", "whether the region pass is needed"),
    (
        "regions.rs",
        "allocate_with_sections",
        "region placement and applied owners",
    ),
    ("relations/mod.rs", "begin", "final relation dispatch"),
    (
        "relations/resolve.rs",
        "Resolver::resolve",
        "final-pass target resolution",
    ),
    ("relations/alignment.rs", "run", "final-pass alignment"),
    (
        "relations/alignment.rs",
        "refresh_guides",
        "final-pass ordered relations",
    ),
    (
        "sections/mod.rs",
        "Table::resolve",
        "FloatSpaces in resolved frame/position inputs",
    ),
];

const READS: &[&str] = &["relations", "resolve_relation", "dead_relations"];

/// Parse Rust rather than match a particular spelling or whitespace layout.
/// Qualified paths include function values and type aliases, not just calls.
/// Imports are rejected at the binding site, before a renamed access can hide
/// the original name. Macro bodies are opaque to syn, so inspect their tokens
/// conservatively too (literals and comments are not identifiers).
#[derive(Default)]
struct Reads {
    scope: Vec<String>,
    found: Vec<(String, String)>,
}

impl Reads {
    fn record(&mut self, name: &str) {
        let name = name.strip_prefix("r#").unwrap_or(name);
        if READS.contains(&name) {
            self.found.push((self.scope.join("::"), name.to_owned()));
        }
    }

    fn macro_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Ident(name) => self.record(&name.to_string()),
                proc_macro2::TokenTree::Group(group) => self.macro_tokens(group.stream()),
                _ => {}
            }
        }
    }

    fn imports(&mut self, tree: &syn::UseTree, parent: &str) {
        match tree {
            syn::UseTree::Path(path) => self.imports(&path.tree, &path.ident.to_string()),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.imports(item, parent);
                }
            }
            syn::UseTree::Name(name) => {
                let ident = name.ident.to_string();
                self.record(if name.ident == "self" { parent } else { &ident });
            }
            syn::UseTree::Rename(rename) => {
                let ident = rename.ident.to_string();
                self.record(if rename.ident == "self" {
                    parent
                } else {
                    &ident
                });
            }
            syn::UseTree::Glob(_) => self.record(parent),
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for Reads {
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        // A trait impl or an unfamiliar self type must not borrow an inherent
        // method's exemption just because its method has the same name.
        let owner = match (&item.trait_, item.self_ty.as_ref()) {
            (None, syn::Type::Path(path))
                if path.qself.is_none() && path.path.segments.len() == 1 =>
            {
                path.path.segments[0].ident.to_string()
            }
            _ => "<other impl>".to_owned(),
        };
        self.scope.push(owner);
        syn::visit::visit_item_impl(self, item);
        self.scope.pop();
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.scope.push(item.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, item);
        self.scope.pop();
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.scope.push(item.sig.ident.to_string());
        syn::visit::visit_item_fn(self, item);
        self.scope.pop();
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.attrs.iter().any(|attr| {
            attr.path().is_ident("cfg")
                && attr
                    .meta
                    .require_list()
                    .is_ok_and(|list| list.tokens.to_string() == "test")
        }) {
            return;
        }
        self.scope.push(item.ident.to_string());
        syn::visit::visit_item_mod(self, item);
        self.scope.pop();
    }

    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        self.record(&expr.method.to_string());
        syn::visit::visit_expr_method_call(self, expr);
    }

    fn visit_expr_path(&mut self, expr: &'ast syn::ExprPath) {
        if expr.qself.is_some() || expr.path.segments.len() > 1 {
            self.record(&expr.path.segments.last().unwrap().ident.to_string());
        }
        syn::visit::visit_expr_path(self, expr);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        // Reject imports even inside allowed readers, so aliases cannot escape
        // the function allowlist. Handle grouped and `self as alias` imports too.
        let scope = std::mem::take(&mut self.scope);
        self.imports(&item.tree, "");
        self.scope = scope;
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.macro_tokens(mac.tokens.clone());
    }
}

fn relation_reads(code: &str) -> Vec<(String, String)> {
    use syn::visit::Visit;
    let parsed = syn::parse_file(code).expect("guard input must be valid Rust syntax");
    let mut reads = Reads::default();
    reads.visit_file(&parsed);
    reads.found
}

fn allowed(file: &str, function: &str) -> bool {
    READERS
        .iter()
        .any(|(path, name, _)| *path == file && *name == function)
}

/// Names that must not come back: a set of owners held by the flow.
const GONE: &[&str] = &["region_owners", "regions::owners("];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

/// `(path relative to src, code lines with comments dropped)`.
fn code() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 30, "found {} sources", files.len());
    files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            (rel, code)
        })
        .collect()
}

#[test]
fn only_listed_places_read_the_relations_of_a_document() {
    let mut reading = Vec::new();
    for (file, code) in code() {
        for (function, read) in relation_reads(&code) {
            assert!(
                allowed(&file, &function),
                "{file}: {function} accesses `{read}` outside READERS; composition must ask Ownership::class"
            );
            reading.push((file.clone(), function));
        }
    }
    let stale: Vec<_> = READERS
        .iter()
        .filter(|(file, function, _)| {
            !reading
                .iter()
                .any(|(f, name)| f == file && name == function)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "{stale:?} no longer read relations: drop them from READERS"
    );
}

#[test]
fn no_flow_holds_a_set_of_owners() {
    for (file, code) in code() {
        for gone in GONE {
            assert!(
                !code.contains(gone),
                "{file} uses `{gone}`: composition learns ownership from `Ownership` (`ownership.rs`) alone"
            );
        }
        if file != "ownership.rs" && file != "running/compose.rs" {
            assert!(
                !code.contains("Classes::read("),
                "{file} reads the classes of the document: only the body's flow (`Ownership::of`) \
                 and a story's digest may"
            );
        }
        if file != "flow.rs" && file != "ownership.rs" {
            assert!(
                !code.contains("Ownership::of("),
                "{file} makes an `Ownership` of the document: only the flow's constructor does"
            );
        }
    }
}

// Exercise exactly the guard's recognition/allowlist logic on an in-memory
// proposed edit. Production source is never changed.
fn accepted_by_guard(file: &str, code: &str) -> bool {
    relation_reads(code)
        .iter()
        .all(|(function, _)| allowed(file, function))
        && !GONE.iter().any(|gone| code.contains(gone))
        && (file == "ownership.rs"
            || file == "running/compose.rs"
            || !code.contains("Classes::read("))
        && (file == "flow.rs" || file == "ownership.rs" || !code.contains("Ownership::of("))
}

#[test]
fn guard_rejects_a_new_live_ownership_read_in_prepare_ahead() {
    let (_, source) = code()
        .into_iter()
        .find(|(file, _)| file == "incremental.rs")
        .unwrap();
    let original = "if doc.kind_of(node) != Some(BlockKind::Paragraph)";
    assert_eq!(source.matches(original).count(), 1);
    let bypass = "if doc.relations().iter().any(|(_, r)| r.as_ref().is_ok_and(|r| r.owner == Some(node))) || doc.kind_of(node) != Some(BlockKind::Paragraph)";
    let proposed = source.replace(original, bypass);
    assert!(
        !accepted_by_guard("incremental.rs", &proposed),
        "guard accepts prepare_ahead bypass"
    );
    assert!(
        !accepted_by_guard("flow/paragraph.rs", "fn probe() { doc.relations(); }"),
        "control: guard must detect an ordinary direct read outside its allowlist"
    );
}

#[test]
fn guard_rejects_ufcs_relation_reads() {
    let bypass = "let owned = reprise_doc::Document::relations(doc).iter().any(|(_, r)| r.as_ref().is_ok_and(|r| r.owner == Some(node)));";
    assert!(
        !accepted_by_guard("flow/paragraph.rs", &format!("fn probe() {{ {bypass} }}")),
        "guard accepts associated-function bypass"
    );
    assert!(!accepted_by_guard(
        "flow/paragraph.rs",
        &format!(
            "fn probe() {{ {} }}",
            bypass.replace("reprise_doc::Document", "Document")
        )
    ));
}

#[test]
fn guard_recognizes_access_spellings_and_function_aliases() {
    for read in READS {
        for expression in [
            format!("doc . {read} ()"),
            format!("doc /* comment */ . r#{read} ()"),
            format!("Document :: {read}(doc)"),
            format!("<reprise_doc::Document>::{read}(doc)"),
            format!("<D as Reader>::{read}(doc)"),
            format!("Document::{read}"),
            format!("{{ let alias = Document::{read}; alias(doc) }}"),
            format!("{{ use reprise_doc::Document as D; D::{read}(doc) }}"),
            format!("{{ type D = reprise_doc::Document; D::{read}(doc) }}"),
        ] {
            let source = format!("fn compose() {{ let _ = {expression}; }}");
            assert!(
                !accepted_by_guard("flow/paragraph.rs", &source),
                "accepted {source}"
            );
        }
    }
}

#[test]
fn guard_rejects_imports_before_aliases_can_hide_access() {
    for read in READS {
        for import in [
            format!("use reader::{read};"),
            format!("use reader::{read} as scan;"),
            format!("use reader::{{{read} as scan}};"),
            format!("use reader::{read}::{{self as scan}};"),
            format!("use reader::{read}::*;"),
        ] {
            assert!(
                !accepted_by_guard("flow/paragraph.rs", &import),
                "accepted {import}"
            );
            let allowed_reader = format!("impl PassKey {{ fn new() {{ {import} }} }}");
            assert!(
                !accepted_by_guard("incremental.rs", &allowed_reader),
                "accepted {allowed_reader}"
            );
        }
    }
}

#[test]
fn guard_exempts_only_the_named_reader_function() {
    assert!(accepted_by_guard(
        "incremental.rs",
        "impl PassKey { fn new() { doc.relations(); } }"
    ));
    for source in [
        "impl Evaluation { fn new() { doc.relations(); } }",
        "impl PassKey { fn prepare_ahead() { doc.relations(); } }",
        "impl PassKey { fn new() {} fn other() { doc.relations(); } }",
        "impl PassKey { fn new() { fn nested() { doc.relations(); } } }",
        "mod nested { impl PassKey { fn new() { doc.relations(); } } }",
        "impl Other for PassKey { fn new() { doc.relations(); } }",
        "impl other::PassKey { fn new() { doc.relations(); } }",
        "const READ: fn(&Document) = Document::relations;",
    ] {
        assert!(
            !accepted_by_guard("incremental.rs", source),
            "accepted {source}"
        );
    }
}

#[test]
fn guard_checks_opaque_macro_tokens_but_ignores_literals_and_comments() {
    for source in [
        "fn compose() { assert!(doc.relations().is_empty()); }",
        "fn compose() { wrap!({ Document::relations(doc) }); }",
        "macro_rules! read { () => { doc.relations() }; }",
        "macro_rules! read { () => { use reader::relations as scan; }; }",
    ] {
        assert!(
            !accepted_by_guard("flow/paragraph.rs", source),
            "accepted {source}"
        );
    }
    assert!(accepted_by_guard(
        "flow/paragraph.rs",
        r##"
        /* doc.relations(); /* nested comment */ Document::relations(doc); */
        fn compose() {
            let _ = "doc.relations()";
            let _ = r#"Document::relations(doc)"#;
            log!("relations");
        }
    "##
    ));
}
