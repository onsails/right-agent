//! AST-level guard keeping production codegen free of database openers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{ExprPath, ItemUse, UseTree};

fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).expect("read right-codegen source directory") {
        let entry = entry.expect("read right-codegen source entry");
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

fn is_test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        let syn::Meta::List(list) = &attribute.meta else {
            return false;
        };
        list.path.is_ident("cfg") && list.tokens.to_string().replace(' ', "") == "test"
    })
}

#[derive(Default)]
struct Aliases {
    targets: HashMap<String, Vec<Vec<String>>>,
}

impl Aliases {
    fn insert(&mut self, alias: String, target: Vec<String>) {
        self.targets.entry(alias).or_default().push(target);
    }

    fn resolved_paths(&self, path: &syn::Path) -> Vec<Vec<String>> {
        let initial: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let mut resolved = Vec::new();
        let mut pending = vec![initial];
        let mut visited = std::collections::HashSet::new();

        while let Some(segments) = pending.pop() {
            if segments.is_empty() || !visited.insert(segments.clone()) {
                continue;
            }
            if let Some(targets) = self.targets.get(&segments[0]) {
                for target in targets {
                    let mut expanded = target.clone();
                    expanded.extend_from_slice(&segments[1..]);
                    pending.push(expanded);
                }
            }
            resolved.push(segments);
        }
        resolved
    }
}

fn collect_use_tree(tree: &UseTree, prefix: &mut Vec<String>, aliases: &mut Aliases) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(&path.tree, prefix, aliases);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut target = prefix.clone();
            target.push(name.ident.to_string());
            aliases.insert(name.ident.to_string(), target);
        }
        UseTree::Rename(rename) => {
            let mut target = prefix.clone();
            target.push(rename.ident.to_string());
            aliases.insert(rename.rename.to_string(), target);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix, aliases);
            }
        }
        UseTree::Glob(_) => {}
    }
}

#[derive(Default)]
struct AliasCollector {
    aliases: Aliases,
}

impl<'ast> Visit<'ast> for AliasCollector {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        collect_use_tree(&item.tree, &mut Vec::new(), &mut self.aliases);
    }
}

fn is_forbidden_path(path: &[String]) -> bool {
    let Some(last) = path.last() else {
        return false;
    };
    let right_db_opener = path.iter().any(|segment| segment == "right_db")
        && (last == "open_db"
            || last.starts_with("open_connection")
            || last.starts_with("open_database_path"));
    let provider_store_opener = last == "open"
        && path
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|owner| owner == "ProviderStore");
    right_db_opener || provider_store_opener
}

struct ForbiddenPathFinder<'a> {
    aliases: &'a Aliases,
    paths: Vec<String>,
}

impl<'ast> Visit<'ast> for ForbiddenPathFinder<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_expr_path(&mut self, expression: &'ast ExprPath) {
        for path in self.aliases.resolved_paths(&expression.path) {
            if is_forbidden_path(&path) {
                self.paths.push(path.join("::"));
            }
        }
        syn::visit::visit_expr_path(self, expression);
    }
}

#[test]
fn production_codegen_has_no_database_openers() {
    let mut files = Vec::new();
    rust_files(&source_root(), &mut files);
    let mut violations = Vec::new();

    for path in files {
        let source = std::fs::read_to_string(&path).expect("read right-codegen source file");
        let syntax = syn::parse_file(&source).expect("parse right-codegen source file");
        let mut collector = AliasCollector::default();
        collector.visit_file(&syntax);
        let mut finder = ForbiddenPathFinder {
            aliases: &collector.aliases,
            paths: Vec::new(),
        };
        finder.visit_file(&syntax);
        for forbidden in finder.paths {
            violations.push(format!("{}: {forbidden}", path.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "right-codegen only generates files; database creation and migration belong to guarded offline CLI paths or the Aggregator owner:\n{}",
        violations.join("\n")
    );
}

#[test]
fn policy_resolves_import_aliases() {
    let syntax = syn::parse_file(
        r#"
        use right_db as database;
        use database::open_connection as open;
        use right_providers::ProviderStore as Store;
        use Store as AliasedStore;
        fn forbidden() {
            open(".", true);
            AliasedStore::open(".");
        }
        "#,
    )
    .unwrap();
    let mut collector = AliasCollector::default();
    collector.visit_file(&syntax);
    let mut finder = ForbiddenPathFinder {
        aliases: &collector.aliases,
        paths: Vec::new(),
    };
    finder.visit_file(&syntax);

    assert_eq!(
        finder.paths,
        [
            "right_db::open_connection",
            "right_providers::ProviderStore::open"
        ]
    );
}
